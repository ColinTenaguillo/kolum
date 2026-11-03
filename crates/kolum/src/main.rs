use kolum::config::{self, Profile};
use kolum::keymap::Keymap;
use kolum::ui::{App, Settings};
use kolum_domain::Provider;
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

#[derive(Default)]
struct Args {
    demo: bool,
    profile: Option<String>,
    config: Option<PathBuf>,
    debug: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--debug" => args.debug = true,
            "-p" | "--profile" => args.profile = Some(it.next().ok_or("--profile needs a name")?),
            "-c" | "--config" => {
                args.config = Some(it.next().ok_or("--config needs a path")?.into())
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("kolum {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(args)
}

type Build = fn(&Profile) -> anyhow::Result<Arc<dyn Provider>>;

/// The providers this build knows, by the name a profile gives. Adding one
/// is a crate and a line here; nothing else changes.
const PROVIDERS: &[(&str, Build)] =
    &[("fake", |_| Ok(Arc::new(kolum_provider_fake::FakeProvider::new())))];

fn provider(args: &Args, config: &config::Config) -> anyhow::Result<Arc<dyn Provider>> {
    if args.demo {
        return Ok(Arc::new(kolum_provider_fake::FakeProvider::new()));
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
        Some((_, build)) => build(profile),
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
    let default = if debug { "kolum=debug,kolum_app=debug" } else { "kolum=info,kolum_app=info" };
    tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default.into()),
        )
        .init();
    Some(dir.join("kolum.log"))
}

fn main() {
    if let Err(e) = app() {
        eprintln!("kolum: {e:#}");
        std::process::exit(1);
    }
}

fn app() -> anyhow::Result<()> {
    let args = parse_args().map_err(anyhow::Error::msg)?;
    let log = init_logging(args.debug);
    if args.debug {
        eprintln!(
            "kolum: logging to {}",
            log.as_ref().map_or("nowhere".into(), |p| p.display().to_string())
        );
    }
    let config_dir = config::config_dir();
    let config_path =
        args.config.clone().or_else(|| config_dir.as_ref().map(|d| d.join("config.toml")));
    let (config, mut problems) = match &config_path {
        Some(path) => config::load(path),
        None => (config::Config::default(), vec![]),
    };
    let (keymap, key_problems) = Keymap::from_config(&config.keys);
    problems.extend(key_problems);
    for p in &problems {
        tracing::warn!("config: {p}");
    }
    let provider = provider(&args, &config)?;
    tracing::info!("starting on the {} provider", provider.name());
    let settings = Settings {
        keymap,
        theme_name: config.ui.theme.clone(),
        themes_dir: config_dir.map(|d| d.join("themes")),
        theme_overrides: config.theme.overrides.clone(),
        ui: config.ui.clone(),
        picker_layout: config.picker.layout,
        problems,
    };

    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async move {
        let (service, events) = kolum_app::Service::spawn(provider);
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
        let result = kolum::run::run(&mut terminal, app, service, events, wake).await;
        ratatui::restore();
        result
    });
    if args.debug
        && let Some(log) = log
    {
        eprintln!("kolum: the log is at {}", log.display());
    }
    result
}
