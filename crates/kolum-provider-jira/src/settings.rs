//! A `provider = "jira"` profile, read and checked.

use crate::map::FieldIds;
use base64::Engine;

/// Cloud and Data Center differ in API version, authentication and rich
/// text format. Read from the profile, or guessed from the URL: Atlassian
/// hosts every Cloud site under `*.atlassian.net`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraFlavor {
    Cloud,
    DataCenter,
}

impl JiraFlavor {
    pub fn api(self) -> &'static str {
        match self {
            JiraFlavor::Cloud => "/rest/api/3",
            JiraFlavor::DataCenter => "/rest/api/2",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JiraSettings {
    pub base_url: String,
    pub flavor: JiraFlavor,
    /// The Authorization header: Basic e-mail and API token on Cloud, a
    /// Bearer personal token on Data Center.
    pub auth: String,
    /// The board to open; the first one listed when absent.
    pub board: Option<u64>,
    pub fields: FieldIds,
}

const KNOWN: &[&str] = &["base_url", "email", "token", "token_cmd", "board", "flavor", "fields"];

/// `token` in the profile, else the output of `token_cmd`, else
/// `$KOLUM_TOKEN`. `run` runs a command and returns its output, so the
/// tests need no shell.
pub fn parse(
    table: &toml::Table,
    env_token: Option<String>,
    run: impl Fn(&str) -> Result<String, String>,
) -> Result<JiraSettings, String> {
    let unknown: Vec<&str> =
        table.keys().map(String::as_str).filter(|k| !KNOWN.contains(k)).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown setting(s) {} (known: {})",
            unknown.join(", "),
            KNOWN.join(", ")
        ));
    }
    let text = |key: &str| table.get(key).and_then(toml::Value::as_str).map(str::to_string);
    let base_url =
        text("base_url").ok_or("`base_url` is required")?.trim_end_matches('/').to_string();
    let flavor = match text("flavor").as_deref() {
        Some("cloud") => JiraFlavor::Cloud,
        Some("datacenter" | "server") => JiraFlavor::DataCenter,
        Some(other) => return Err(format!("flavor `{other}`: expected cloud or datacenter")),
        None if base_url.contains(".atlassian.net") => JiraFlavor::Cloud,
        None => JiraFlavor::DataCenter,
    };
    let token = match (text("token"), text("token_cmd")) {
        (Some(token), _) => token,
        (None, Some(cmd)) => {
            run(&cmd).map_err(|e| format!("token_cmd `{cmd}`: {e}"))?.trim().to_string()
        }
        (None, None) => env_token.ok_or("no token: set `token_cmd`, `token`, or $KOLUM_TOKEN")?,
    };
    let auth = match flavor {
        JiraFlavor::Cloud => {
            let email = text("email").ok_or("`email` is required on Jira Cloud")?;
            let pair = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));
            format!("Basic {pair}")
        }
        JiraFlavor::DataCenter => format!("Bearer {token}"),
    };
    let board = match table.get("board") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|n| u64::try_from(n).ok())
                .ok_or("`board` must be a board id")?,
        ),
    };
    let forced = table.get("fields").and_then(toml::Value::as_table);
    let field =
        |k: &str| forced.and_then(|f| f.get(k)).and_then(toml::Value::as_str).map(String::from);
    let fields = FieldIds {
        estimate: field("estimate"),
        sprint: field("sprint"),
        epic_link: field("epic_link"),
    };
    Ok(JiraSettings { base_url, flavor, auth, board, fields })
}

/// Runs `token_cmd` through the shell, as a password manager expects.
pub fn run_command(cmd: &str) -> Result<String, String> {
    let out =
        std::process::Command::new("sh").arg("-c").arg(cmd).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("exited with {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| "printed something that is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    fn no_shell(_: &str) -> Result<String, String> {
        Err("no shell in tests".into())
    }

    #[test]
    fn a_cloud_profile_uses_basic_auth_with_the_email() {
        let s = parse(
            &table("base_url = \"https://acme.atlassian.net/\"\nemail = \"me@acme.com\"\ntoken = \"t0k\"\nboard = 42"),
            None,
            no_shell,
        )
        .unwrap();
        assert_eq!(s.base_url, "https://acme.atlassian.net");
        assert_eq!(s.flavor, JiraFlavor::Cloud);
        assert_eq!(s.auth, "Basic bWVAYWNtZS5jb206dDBr");
        assert_eq!(s.board, Some(42));
    }

    #[test]
    fn a_self_hosted_url_is_data_center_with_a_bearer_token() {
        let s =
            parse(&table("base_url = \"https://jira.acme.corp\""), Some("pat".into()), no_shell)
                .unwrap();
        assert_eq!(s.flavor, JiraFlavor::DataCenter);
        assert_eq!(s.auth, "Bearer pat");
        assert_eq!(s.flavor.api(), "/rest/api/2");
    }

    #[test]
    fn the_token_comes_from_the_file_then_the_command_then_the_environment() {
        let run = |cmd: &str| Ok(format!("{cmd}-out\n"));
        let base = "base_url = \"https://x.corp\"\n";
        let s = parse(
            &table(&format!("{base}token = \"file\"\ntoken_cmd = \"pass\"")),
            Some("env".into()),
            run,
        );
        assert_eq!(s.unwrap().auth, "Bearer file");
        let s = parse(&table(&format!("{base}token_cmd = \"pass\"")), Some("env".into()), run);
        assert_eq!(s.unwrap().auth, "Bearer pass-out");
        let s = parse(&table(base), Some("env".into()), run);
        assert_eq!(s.unwrap().auth, "Bearer env");
    }

    #[test]
    fn mistakes_are_named() {
        let err = |src: &str| parse(&table(src), Some("t".into()), no_shell).unwrap_err();
        assert_eq!(err(""), "`base_url` is required");
        assert!(err("base_url = \"https://x\"\nbord = 4").starts_with("unknown setting(s) bord"));
        assert_eq!(
            err("base_url = \"https://a.atlassian.net\""),
            "`email` is required on Jira Cloud"
        );
        assert_eq!(
            err("base_url = \"https://x\"\nflavor = \"saas\""),
            "flavor `saas`: expected cloud or datacenter"
        );
        assert_eq!(err("base_url = \"https://x\"\nboard = \"main\""), "`board` must be a board id");
        let failing =
            parse(&table("base_url = \"https://x\"\ntoken_cmd = \"pass\""), None, no_shell);
        assert_eq!(failing.unwrap_err(), "token_cmd `pass`: no shell in tests");
        assert_eq!(
            parse(&table("base_url = \"https://x\""), None, no_shell).unwrap_err(),
            "no token: set `token_cmd`, `token`, or $KOLUM_TOKEN"
        );
    }

    #[test]
    fn fields_can_be_forced() {
        let s = parse(
            &table("base_url = \"https://x\"\ntoken = \"t\"\n[fields]\nestimate = \"customfield_1\"\nsprint = \"customfield_2\""),
            None,
            no_shell,
        )
        .unwrap();
        assert_eq!(s.fields.estimate.as_deref(), Some("customfield_1"));
        assert_eq!(s.fields.sprint.as_deref(), Some("customfield_2"));
        assert_eq!(s.fields.epic_link, None);
    }

    #[test]
    fn run_command_returns_stdout_or_the_failure() {
        assert_eq!(run_command("printf secret").unwrap(), "secret");
        assert!(run_command("exit 3").unwrap_err().contains("exited"));
    }
}
