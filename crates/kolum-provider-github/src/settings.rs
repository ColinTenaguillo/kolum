//! A `provider = "github"` profile.

#[derive(Debug, Clone, PartialEq)]
pub struct GithubSettings {
    /// The GraphQL endpoint: github.com's, or a GitHub Enterprise one.
    pub api_url: String,
    pub owner: String,
    pub number: u64,
    pub token: String,
    /// The single-select field whose options are the board's columns.
    pub status_field: String,
}

/// `token_cmd` is not here: the binary runs it and hands over `token`.
const KNOWN: &[&str] = &["project", "token", "status_field", "api_url"];

/// `project = "owner/12"`; the token from the profile (or its
/// `token_cmd`, already run), `$KOLUM_TOKEN` or `$GITHUB_TOKEN`.
pub fn parse(table: &toml::Table, env_token: Option<String>) -> Result<GithubSettings, String> {
    let unknown: Vec<&str> =
        table.keys().map(String::as_str).filter(|k| !KNOWN.contains(k)).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown setting(s) {} (known: {})",
            unknown.join(", "),
            KNOWN.join(", ")
        ));
    }
    let text = |k: &str| table.get(k).and_then(toml::Value::as_str).map(str::to_string);
    let project = text("project").ok_or("`project` is required, as owner/number")?;
    let (owner, number) = project
        .split_once('/')
        .and_then(|(o, n)| Some((o.to_string(), n.parse::<u64>().ok()?)))
        .filter(|(o, _)| !o.is_empty())
        .ok_or_else(|| format!("project `{project}`: expected owner/number, like octo-org/5"))?;
    let token = text("token")
        .or(env_token)
        .ok_or("no token: set `token_cmd` (`gh auth token`), `token`, or $GITHUB_TOKEN")?;
    Ok(GithubSettings {
        api_url: text("api_url").unwrap_or_else(|| "https://api.github.com/graphql".into()),
        owner,
        number,
        token,
        status_field: text("status_field").unwrap_or_else(|| "Status".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(src: &str, env: Option<&str>) -> Result<GithubSettings, String> {
        parse(&toml::from_str(src).unwrap(), env.map(String::from))
    }

    #[test]
    fn a_profile_names_its_project_and_token() {
        let s = parsed("project = \"octo-org/5\"\ntoken = \"gho\"", Some("env")).unwrap();
        assert_eq!(
            (s.owner.as_str(), s.number, s.token.as_str()),
            ("octo-org", 5, "gho"),
            "the profile's first"
        );
        assert_eq!(s.status_field, "Status");
        assert_eq!(s.api_url, "https://api.github.com/graphql");
        let s = parsed("project = \"me/1\"\nstatus_field = \"Stage\"", Some("env")).unwrap();
        assert_eq!((s.token.as_str(), s.status_field.as_str()), ("env", "Stage"));
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(parsed("", Some("t")).unwrap_err(), "`project` is required, as owner/number");
        assert!(
            parsed("project = \"octo\"", Some("t")).unwrap_err().contains("expected owner/number")
        );
        assert!(
            parsed("project = \"/3\"", Some("t")).unwrap_err().contains("expected owner/number")
        );
        assert!(
            parsed("project = \"a/1\"\nboard = 3", Some("t"))
                .unwrap_err()
                .starts_with("unknown setting(s) board")
        );
        assert!(parsed("project = \"a/1\"", None).unwrap_err().starts_with("no token"));
    }
}
