use kolum::config::{self, Profile};
use kolum::keymap::Keymap;
use kolum::ui::{App, Settings};
use kolum_domain::Provider;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

const USAGE: &str = "\
kolum: a kanban client for the terminal

Usage: kolum [options]

  --demo               Run on built-in demo data, no account needed
  -p, --profile NAME   Use this profile from the config
  -c, --config PATH    Read this config instead of ~/.config/kolum/config.toml
  --debug              Log at debug level, and print where the log is
  -h, --help           This help
  -V, --version        The version";

#[derive(Debug, Default, PartialEq)]
struct Args {
    demo: bool,
    profile: Option<String>,
    config: Option<PathBuf>,
    debug: bool,
}

#[derive(Debug, PartialEq)]
enum Cli {
    Run(Args),
    Help,
    Version,
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut args = Args::default();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--debug" => args.debug = true,
            "-p" | "--profile" => args.profile = Some(it.next().ok_or("--profile needs a name")?),
            "-c" | "--config" => {
                args.config = Some(it.next().ok_or("--config needs a path")?.into())
            }
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(Cli::Run(args))
}

type Build = fn(&Profile) -> anyhow::Result<Arc<dyn Provider>>;

/// The providers this build knows, by the name a profile gives. Adding one
/// is a crate and a line here; nothing else changes.
const PROVIDERS: &[(&str, Build)] = &[
    ("fake", |_| Ok(Arc::new(kolum_provider_fake::FakeProvider::new()))),
    ("jira", |p| {
        let jira = kolum_provider_jira::JiraProvider::from_profile(&p.settings)
            .map_err(anyhow::Error::msg)?;
        Ok(Arc::new(jira))
    }),
    ("github", |p| {
        let github = kolum_provider_github::GithubProvider::from_profile(&p.settings)
            .map_err(anyhow::Error::msg)?;
        Ok(Arc::new(github))
    }),
];

/// Runs a profile's `token_cmd`, once, for every provider: the secret is
/// handled here and nowhere else, and the adapter reads only `token`.
fn with_token(
    profile: &Profile,
    run: impl Fn(&str) -> Result<String, String>,
) -> Result<Profile, String> {
    let mut profile = profile.clone();
    if let Some(cmd) = profile.settings.remove("token_cmd") {
        let cmd = cmd.as_str().ok_or("`token_cmd` must be a command in quotes")?.to_string();
        if !profile.settings.contains_key("token") {
            let token = run(&cmd).map_err(|e| format!("`token_cmd`: {e}"))?;
            profile.settings.insert("token".into(), toml::Value::String(token.trim().to_string()));
        }
    }
    Ok(profile)
}

/// Runs `token_cmd` through the shell, as a password manager expects.
/// Errors never quote the command: it may be a token pasted in its place.
fn run_token_cmd(cmd: &str) -> Result<String, String> {
    let out =
        std::process::Command::new("sh").arg("-c").arg(cmd).output().map_err(|e| e.to_string())?;
    // 127: the shell's "command not found".
    if out.status.code() == Some(127) {
        return Err(
            "is not a command: to keep the token itself, write `token = \"…\"` instead".into()
        );
    }
    if !out.status.success() {
        return Err(format!("exited with {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| "printed something that is not UTF-8".into())
}

/// The board a profile opens: `board = 42` or `board = "PROJ"`. Without
/// one, the provider's first board.
fn profile_board(profile: &Profile) -> Option<kolum_domain::BoardId> {
    match profile.settings.get("board")? {
        toml::Value::Integer(n) => Some(kolum_domain::BoardId::new(n.to_string())),
        toml::Value::String(s) => Some(kolum_domain::BoardId::new(s)),
        _ => None,
    }
}

/// A provider ready to use, the board its profile names, and the profile's
/// name, which names its cache directory.
type Opened = (Arc<dyn Provider>, Option<kolum_domain::BoardId>, String);

fn provider(args: &Args, config: &config::Config) -> anyhow::Result<Opened> {
    if args.demo {
        return Ok((Arc::new(kolum_provider_fake::FakeProvider::new()), None, "demo".into()));
    }
    let name = args.profile.clone().or_else(|| config.default_profile.clone()).or_else(|| {
        (config.profiles.len() == 1).then(|| config.profiles.keys().next().cloned()).flatten()
    });
    let Some(name) = name else {
        anyhow::bail!(
            "no profile to open.\n\nTry `kolum --demo` for a board of demo data, or add a profile to \
             ~/.config/kolum/config.toml (see the README)."
        );
    };
    let Some(profile) = config.profiles.get(&name) else {
        let known: Vec<_> = config.profiles.keys().cloned().collect();
        anyhow::bail!("no profile `{name}` in the config (it has: {})", known.join(", "));
    };
    match PROVIDERS.iter().find(|(n, _)| *n == profile.provider) {
        Some((_, build)) => {
            let profile = with_token(profile, run_token_cmd)
                .map_err(|e| anyhow::anyhow!("profile `{name}`: {e}"))?;
            let opened = build(&profile).map_err(|e| e.context(format!("profile `{name}`")))?;
            Ok((opened, profile_board(&profile), name.clone()))
        }
        None => {
            let known: Vec<_> = PROVIDERS.iter().map(|(n, _)| *n).collect();
            anyhow::bail!(
                "profile `{name}`: the `{}` provider is not in this build yet (available: {}). \
                 `kolum --demo` runs on demo data.",
                profile.provider,
                known.join(", ")
            )
        }
    }
}

/// File logging: a TUI cannot be debugged with println!, and anything on
/// stdout lands in the middle of the screen.
fn init_logging(debug: bool) -> Option<PathBuf> {
    let dir = config::cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::never(&dir, "kolum.log");
    let level = if debug { "debug" } else { "info" };
    let default = ["kolum", "kolum_app", "kolum_provider_jira", "kolum_provider_github"]
        .map(|target| format!("{target}={level}"))
        .join(",");
    tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default)),
        )
        .init();
    Some(dir.join("kolum.log"))
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some(kolum::images::PROBE_FLAG) {
        kolum::images::probe_child();
    }
    if let Err(e) = app() {
        eprintln!("kolum: {e:#}");
        std::process::exit(1);
    }
}

fn app() -> anyhow::Result<()> {
    let args = match parse_args(std::env::args().skip(1)).map_err(anyhow::Error::msg)? {
        Cli::Run(args) => args,
        Cli::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Cli::Version => {
            println!("kolum {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };
    let log = init_logging(args.debug);
    if args.debug {
        let at = log.as_ref().map_or("nowhere".into(), |p| p.display().to_string());
        eprintln!("kolum: logging to {at}");
    }
    let (args, config, settings) = configure(args)?;
    for p in &settings.problems {
        tracing::warn!("config: {p}");
    }
    let (provider, board, profile) = provider(&args, &config)?;
    tracing::info!("starting on the {} provider", provider.name());
    let writable = provider.capabilities().supports(kolum_domain::Capability::Write);
    let state_file = config::cache_dir().map(|d| d.join(&profile).join("state.toml"));
    let settings = with_state(Settings { board, writable, ..settings }, state_file.as_deref());
    let cache = config::cache_dir().map(|d| {
        Arc::new(kolum::cache::Cache::new(d.join(&profile))) as Arc<dyn kolum_app::Storage>
    });
    // Before raw mode: the probe talks to the terminal.
    let settings = Settings { images: kolum::images::probe(settings.ui.images), ..settings };
    let result = run_tui(provider, cache, state_file, settings);
    if args.debug
        && let Some(log) = log
    {
        eprintln!("kolum: the log is at {}", log.display());
    }
    result
}

/// The config, after the first-run questions when there is no profile yet
/// and someone at the terminal to answer them.
fn configure(mut args: Args) -> anyhow::Result<(Args, config::Config, Settings)> {
    let (config, settings) = load(&args, config::config_dir());
    let first = config.profiles.is_empty() && !args.demo && args.profile.is_none();
    if !first || !std::io::stdin().is_terminal() {
        return Ok((args, config, settings));
    }
    if first_run(&args)? == kolum::wizard::Answer::Demo {
        args.demo = true;
        return Ok((args, config, settings));
    }
    let (config, settings) = load(&args, config::config_dir());
    Ok((args, config, settings))
}

/// The toggles of the last run win over the config's starting values.
fn with_state(mut settings: Settings, state_file: Option<&std::path::Path>) -> Settings {
    if let Some(state) = state_file.and_then(config::UiState::load) {
        settings.ui.hide_empty_columns = state.hide_empty_columns;
        settings.ui.swimlanes = state.swimlanes;
        if let Some(filter) = state.applied_filter {
            settings.ui.filter = filter;
            settings.filter_applied = true;
        }
    }
    settings
}

/// Asks the first-run questions and writes the profile they describe.
fn first_run(args: &Args) -> anyhow::Result<kolum::wizard::Answer> {
    let path = args
        .config
        .clone()
        .or_else(|| config::config_dir().map(|d| d.join("config.toml")))
        .ok_or_else(|| anyhow::anyhow!("no home directory to keep the config in"))?;
    let answer = kolum::wizard::questions(&mut std::io::stdin().lock(), &mut std::io::stdout())?;
    if let kolum::wizard::Answer::Jira(jira) = &answer {
        kolum::wizard::write_profile(&path, jira)?;
        println!("\nWrote the `{}` profile to {}.", jira.name, path.display());
    }
    Ok(answer)
}

/// The config, and what the app is built from, with every problem found.
fn load(args: &Args, config_dir: Option<PathBuf>) -> (config::Config, Settings) {
    let path = args.config.clone().or_else(|| config_dir.as_ref().map(|d| d.join("config.toml")));
    let (config, mut problems) = match &path {
        Some(path) => config::load(path),
        None => (config::Config::default(), vec![]),
    };
    let (keymap, key_problems) = Keymap::from_config(&config.keys);
    problems.extend(key_problems);
    let settings = Settings {
        keymap,
        theme_name: config.ui.theme.clone(),
        themes_dir: config_dir.map(|d| d.join("themes")),
        theme_overrides: config.theme.overrides.clone(),
        ui: config.ui.clone(),
        picker_layout: config.picker.layout,
        problems,
        board: None,
        writable: false,
        images: None,
        remote_debounce: std::time::Duration::from_millis(config.picker.remote_debounce_ms),
        filter_applied: false,
    };
    (config, settings)
}

fn run_tui(
    provider: Arc<dyn Provider>,
    cache: Option<Arc<dyn kolum_app::Storage>>,
    state_file: Option<PathBuf>,
    settings: Settings,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let (service, events) = kolum_app::Service::spawn(provider, cache);
        let (wake_tx, wake) = tokio::sync::mpsc::unbounded_channel();
        // Fails only once the loop has ended, when no one needs waking.
        let app = App::new(
            settings,
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
        );
        // try_init() rather than init(): the latter panics when there is no
        // terminal, and a backtrace is a poor way to say "I need one".
        let mut terminal = ratatui::try_init().map_err(|e| {
            anyhow::anyhow!("kolum needs an interactive terminal, and could not open one: {e}")
        })?;
        // try_init's panic hook restores the terminal. Log first: the
        // restore repaints, wiping a message printed before it, and a crash
        // would look like a clean exit. Then exit, since a panic on a tokio
        // task does not end the process and the loop would keep drawing
        // over a restored terminal.
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!("panic: {info}");
            restore(info);
            std::process::exit(1);
        }));
        let result = kolum::run::run(state_file, &mut terminal, app, service, events, wake).await;
        ratatui::restore();
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Cli, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn arguments() {
        assert_eq!(args(""), Ok(Cli::Run(Args::default())));
        let all = Args {
            demo: true,
            debug: true,
            profile: Some("perso".into()),
            config: Some("/tmp/k.toml".into()),
        };
        assert_eq!(args("--demo --debug -p perso -c /tmp/k.toml"), Ok(Cli::Run(all)));
        assert_eq!(args("--profile a --config b").map(|c| matches!(c, Cli::Run(_))), Ok(true));
        assert_eq!(args("--demo -h"), Ok(Cli::Help));
        assert_eq!(args("-V"), Ok(Cli::Version));
        assert_eq!(args("-p"), Err("--profile needs a name".into()));
        assert_eq!(args("--config"), Err("--config needs a path".into()));
        assert!(args("--nope").unwrap_err().starts_with("unknown argument `--nope`"));
    }

    fn config(src: &str) -> config::Config {
        config::parse(src).0
    }

    fn opened(args: Args, src: &str) -> Result<String, String> {
        provider(&args, &config(src))
            .map(|(p, _, _)| p.name().to_string())
            .map_err(|e| format!("{e:#}"))
    }

    #[test]
    fn the_profile_picks_the_provider() {
        let demo = Args { demo: true, ..Args::default() };
        assert_eq!(opened(demo, ""), Ok("fake".into()), "--demo needs no config");
        let one = "[profiles.only]\nprovider = \"fake\"";
        assert_eq!(
            opened(Args::default(), one),
            Ok("fake".into()),
            "a lone profile is the default"
        );
        let two = "default_profile = \"b\"\n[profiles.a]\nprovider = \"x\"\n[profiles.b]\nprovider = \"fake\"";
        assert_eq!(opened(Args::default(), two), Ok("fake".into()));
        let pick_a = Args { profile: Some("a".into()), ..Args::default() };
        assert!(opened(pick_a, two).unwrap_err().contains("the `x` provider is not in this build"));
        let missing = Args { profile: Some("zz".into()), ..Args::default() };
        assert_eq!(
            opened(missing, two),
            Err("no profile `zz` in the config (it has: a, b)".into())
        );
        assert!(opened(Args::default(), "").unwrap_err().starts_with("no profile to open"));
    }

    #[test]
    fn a_jira_profile_is_checked_and_opens_its_board() {
        let jira = "[profiles.work]\nprovider = \"jira\"\nbase_url = \"https://acme.atlassian.net\"\n\
                    email = \"me@acme.com\"\ntoken = \"t\"\nboard = 42";
        let (p, board, profile) = provider(&Args::default(), &config(jira)).unwrap();
        assert_eq!(profile, "work");
        assert_eq!(p.name(), "jira");
        assert_eq!(board, Some(kolum_domain::BoardId::new("42")));
        let broken = "[profiles.work]\nprovider = \"jira\"\nbase_url = \"https://acme.atlassian.net\"\ntoken = \"t\"";
        assert_eq!(
            opened(Args::default(), broken),
            Err("profile `work`: `email` is required on Jira Cloud".into())
        );
        let gh = "[profiles.perso]\nprovider = \"github\"\nproject = \"octo/5\"\ntoken = \"t\"";
        assert_eq!(opened(Args::default(), gh), Ok("github".into()));
        let named = "[profiles.x]\nprovider = \"fake\"\nboard = \"MAIN\"";
        assert_eq!(
            provider(&Args::default(), &config(named)).unwrap().1,
            Some(kolum_domain::BoardId::new("MAIN"))
        );
    }

    #[test]
    fn token_cmd_is_run_once_here_and_handed_over_as_token() {
        let profile = |src: &str| config(src).profiles.remove("p").unwrap();
        let echo = |c: &str| Ok(format!("{c}-out\n"));
        let p =
            with_token(&profile("[profiles.p]\nprovider = \"jira\"\ntoken_cmd = \"pass x\""), echo)
                .unwrap();
        assert_eq!(p.settings["token"].as_str(), Some("pass x-out"));
        assert!(!p.settings.contains_key("token_cmd"), "the adapter never sees it");
        let both =
            profile("[profiles.p]\nprovider = \"jira\"\ntoken = \"t\"\ntoken_cmd = \"pass x\"");
        let kept = with_token(&both, |_| Err("not run".into())).unwrap();
        assert_eq!(
            kept.settings["token"].as_str(),
            Some("t"),
            "token wins, and the command is not run"
        );
        let failing = profile("[profiles.p]\nprovider = \"jira\"\ntoken_cmd = \"pass x\"");
        assert_eq!(
            with_token(&failing, |_| Err("no shell".into())).unwrap_err(),
            "`token_cmd`: no shell",
            "the command may be a pasted token: it is never shown"
        );
        let bad = profile("[profiles.p]\nprovider = \"jira\"\ntoken_cmd = 3");
        assert!(with_token(&bad, echo).is_err());
        assert_eq!(run_token_cmd("printf secret").unwrap(), "secret");
        assert!(run_token_cmd("exit 3").unwrap_err().contains("exited"));
        assert_eq!(
            run_token_cmd("ATATT3xFfGF0secret").unwrap_err(),
            "is not a command: to keep the token itself, write `token = \"…\"` instead"
        );
    }

    #[test]
    fn the_last_runs_toggles_win_over_the_config() {
        let dir = std::env::temp_dir().join(format!("kolum-with-state-{}", std::process::id()));
        let path = dir.join("state.toml");
        let (_, settings) = load(&Args::default(), None);
        let untouched = with_state(settings, Some(&path));
        assert!(!untouched.ui.hide_empty_columns, "no state yet: the config's");
        let state = config::UiState {
            hide_empty_columns: true,
            swimlanes: config::Swimlanes::Priority,
            applied_filter: Some("@me #bug".into()),
        };
        state.save(&path).unwrap();
        let restored = with_state(untouched, Some(&path));
        assert!(restored.ui.hide_empty_columns);
        assert_eq!(restored.ui.swimlanes, config::Swimlanes::Priority);
        assert_eq!(restored.ui.filter, "@me #bug");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The config's filter starts the board until one is applied: a state
    /// from before, or one where none was ever applied, leaves it.
    #[test]
    fn the_configs_filter_wins_until_one_is_applied() {
        let dir = std::env::temp_dir().join(format!("kolum-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (config, path) = (dir.join("config.toml"), dir.join("state.toml"));
        std::fs::write(&config, "[ui]\nfilter = \"@me\"\n").unwrap();
        let args = Args { config: Some(config), ..Args::default() };
        let toggles = "hide_empty_columns = true\nswimlanes = \"epic\"\n";
        // A previous build wrote `filter = ""` whether one was applied or not.
        for older in [toggles.to_string(), format!("{toggles}filter = \"\"")] {
            std::fs::write(&path, older).unwrap();
            let older = with_state(load(&args, None).1, Some(&path));
            assert_eq!((older.ui.filter.as_str(), older.filter_applied), ("@me", false));
        }
        std::fs::write(&path, format!("{toggles}applied_filter = \"\"")).unwrap();
        let cleared = with_state(load(&args, None).1, Some(&path));
        assert_eq!((cleared.ui.filter.as_str(), cleared.filter_applied), ("", true), "cleared too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_reads_the_given_config_and_gathers_every_problem() {
        let dir = std::env::temp_dir().join(format!("kolum-main-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom.toml");
        std::fs::write(&path, "[ui]\ntheme = \"gruvbox\"\nthme = 1\n[keys.board]\nx = \"nope\"\n")
            .unwrap();
        let args = Args { config: Some(path), ..Args::default() };
        let (config, settings) = load(&args, Some(dir.clone()));
        assert_eq!(config.ui.theme, "gruvbox");
        assert_eq!(settings.theme_name, "gruvbox");
        assert_eq!(settings.themes_dir, Some(dir.join("themes")));
        assert_eq!(
            settings.problems,
            ["unknown setting `ui.thme`", "[keys.board] \"x\": unknown action `nope`"]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
