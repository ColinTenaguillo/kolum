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

/// `token_cmd` is not here: the binary runs it and hands over `token`.
const KNOWN: &[&str] = &["base_url", "email", "token", "board", "flavor", "fields"];

/// `token` in the profile (or its `token_cmd`, already run), else
/// `$KOLUM_TOKEN`.
pub fn parse(table: &toml::Table, env_token: Option<String>) -> Result<JiraSettings, String> {
    let unknown: Vec<&str> =
        table.keys().map(String::as_str).filter(|k| !KNOWN.contains(k)).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown setting(s) {} (known: {})",
            unknown.join(", "),
            KNOWN.join(", ")
        ));
    }
    let base_url =
        text(table, "base_url").ok_or("`base_url` is required")?.trim_end_matches('/').to_string();
    let flavor = flavor(text(table, "flavor").as_deref(), &base_url)?;
    let token = text(table, "token")
        .or(env_token)
        .ok_or("no token: set `token_cmd`, `token`, or $KOLUM_TOKEN")?;
    let auth = auth(flavor, text(table, "email"), &token)?;
    let board = match table.get("board") {
        None => None,
        Some(v) => Some(
            v.as_integer()
                .and_then(|n| u64::try_from(n).ok())
                .ok_or("`board` must be a board id")?,
        ),
    };
    let forced = table.get("fields").and_then(toml::Value::as_table);
    let field = |k: &str| forced.and_then(|f| text(f, k));
    let fields = FieldIds {
        estimate: field("estimate"),
        sprint: field("sprint"),
        epic_link: field("epic_link"),
    };
    Ok(JiraSettings { base_url, flavor, auth, board, fields })
}

fn text(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key).and_then(toml::Value::as_str).map(str::to_string)
}

fn flavor(named: Option<&str>, base_url: &str) -> Result<JiraFlavor, String> {
    match named {
        Some("cloud") => Ok(JiraFlavor::Cloud),
        Some("datacenter" | "server") => Ok(JiraFlavor::DataCenter),
        Some(other) => Err(format!("flavor `{other}`: expected cloud or datacenter")),
        None if base_url.contains(".atlassian.net") => Ok(JiraFlavor::Cloud),
        None => Ok(JiraFlavor::DataCenter),
    }
}

fn auth(flavor: JiraFlavor, email: Option<String>, token: &str) -> Result<String, String> {
    match flavor {
        JiraFlavor::Cloud => {
            let email = email.ok_or("`email` is required on Jira Cloud")?;
            let pair = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));
            Ok(format!("Basic {pair}"))
        }
        JiraFlavor::DataCenter => Ok(format!("Bearer {token}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    fn err(src: &str) -> String {
        parse(&table(src), Some("t".into())).unwrap_err()
    }

    #[test]
    fn a_cloud_profile_uses_basic_auth_with_the_email() {
        let src = "base_url = \"https://acme.atlassian.net/\"\nemail = \"me@acme.com\"\ntoken = \"t0k\"\nboard = 42";
        let s = parse(&table(src), None).unwrap();
        assert_eq!(s.base_url, "https://acme.atlassian.net");
        assert_eq!(s.flavor, JiraFlavor::Cloud);
        assert_eq!(s.auth, "Basic bWVAYWNtZS5jb206dDBr");
        assert_eq!(s.board, Some(42));
    }

    #[test]
    fn a_self_hosted_url_is_data_center_with_a_bearer_token() {
        let s = parse(&table("base_url = \"https://jira.acme.corp\""), Some("pat".into())).unwrap();
        assert_eq!(s.flavor, JiraFlavor::DataCenter);
        assert_eq!(s.auth, "Bearer pat");
        assert_eq!(s.flavor.api(), "/rest/api/2");
    }

    #[test]
    fn the_token_comes_from_the_profile_then_the_environment() {
        let base = "base_url = \"https://x.corp\"\n";
        assert_eq!(
            parse(&table(&format!("{base}token = \"file\"")), Some("env".into())).unwrap().auth,
            "Bearer file"
        );
        assert_eq!(parse(&table(base), Some("env".into())).unwrap().auth, "Bearer env");
        assert_eq!(
            parse(&table(base), None).unwrap_err(),
            "no token: set `token_cmd`, `token`, or $KOLUM_TOKEN"
        );
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(err(""), "`base_url` is required");
        assert!(err("base_url = \"https://x\"\nbord = 4").starts_with("unknown setting(s) bord"));
        assert!(
            err("base_url = \"https://x\"\ntoken_cmd = \"pass\"")
                .starts_with("unknown setting(s) token_cmd")
        );
        assert_eq!(
            err("base_url = \"https://a.atlassian.net\""),
            "`email` is required on Jira Cloud"
        );
        assert_eq!(
            err("base_url = \"https://x\"\nflavor = \"saas\""),
            "flavor `saas`: expected cloud or datacenter"
        );
        assert_eq!(err("base_url = \"https://x\"\nboard = \"main\""), "`board` must be a board id");
        let forced = |f: &str| {
            let src = format!(
                "base_url = \"https://a.atlassian.net\"\nflavor = \"{f}\"\nemail = \"e\"\ntoken = \"t\""
            );
            parse(&table(&src), None).unwrap().flavor
        };
        assert_eq!(forced("server"), JiraFlavor::DataCenter, "a proxy in front of Data Center");
        assert_eq!(forced("cloud"), JiraFlavor::Cloud);
    }

    #[test]
    fn fields_can_be_forced() {
        let src = "base_url = \"https://x\"\ntoken = \"t\"\n[fields]\nestimate = \"customfield_1\"\nsprint = \"customfield_2\"";
        let s = parse(&table(src), None).unwrap();
        assert_eq!(s.fields.estimate.as_deref(), Some("customfield_1"));
        assert_eq!(s.fields.sprint.as_deref(), Some("customfield_2"));
        assert_eq!(s.fields.epic_link, None);
    }
}
