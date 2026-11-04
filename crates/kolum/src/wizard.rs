//! The first run: a few questions on the plain terminal, before the TUI,
//! that write a profile to config.toml.

use std::io::{BufRead, Write};
use std::path::Path;

/// What the questions found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Demo,
    Jira(JiraAnswers),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraAnswers {
    pub name: String,
    pub base_url: String,
    pub email: Option<String>,
    pub token: Token,
    pub board: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Command(String),
    Environment,
    InFile(String),
}

fn ask(
    input: &mut impl BufRead,
    out: &mut impl Write,
    question: &str,
    default: &str,
) -> std::io::Result<String> {
    if default.is_empty() {
        write!(out, "{question}: ")?;
    } else {
        write!(out, "{question} [{default}]: ")?;
    }
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no answer"));
    }
    let line = line.trim();
    Ok(if line.is_empty() { default.to_string() } else { line.to_string() })
}

/// Asks again until `check` accepts the answer.
fn ask_until(
    input: &mut impl BufRead,
    out: &mut impl Write,
    question: &str,
    default: &str,
    check: impl Fn(&str) -> Result<(), &'static str>,
) -> std::io::Result<String> {
    loop {
        let answer = ask(input, out, question, default)?;
        match check(&answer) {
            Ok(()) => return Ok(answer),
            Err(why) => writeln!(out, "  {why}")?,
        }
    }
}

pub fn questions(input: &mut impl BufRead, out: &mut impl Write) -> std::io::Result<Answer> {
    writeln!(out, "kolum has no profile yet. A few questions set one up (Ctrl-C to stop).\n")?;
    let provider = ask_until(
        input,
        out,
        "Provider: jira, or demo to look around first",
        "jira",
        |a| match a {
            "jira" | "demo" => Ok(()),
            _ => Err("jira or demo"),
        },
    )?;
    if provider == "demo" {
        return Ok(Answer::Demo);
    }
    jira_questions(input, out).map(Answer::Jira)
}

fn jira_questions(input: &mut impl BufRead, out: &mut impl Write) -> std::io::Result<JiraAnswers> {
    let base_url = ask_until(input, out, "Jira URL, like https://acme.atlassian.net", "", |a| {
        if a.starts_with("https://") || a.starts_with("http://") {
            Ok(())
        } else {
            Err("a URL, starting with https://")
        }
    })?;
    let base_url = base_url.trim_end_matches('/').to_string();
    let email = if base_url.contains(".atlassian.net") {
        Some(ask_until(input, out, "Your Atlassian e-mail", "", |a| {
            if a.contains('@') { Ok(()) } else { Err("an e-mail address") }
        })?)
    } else {
        None
    };
    let token = token_question(input, out)?;
    let board = ask_until(
        input,
        out,
        "Board id, from the board's URL (blank: the first board)",
        "",
        |a| {
            if a.is_empty() || a.parse::<u64>().is_ok() {
                Ok(())
            } else {
                Err("a number, or blank")
            }
        },
    )?;
    let name = ask_until(input, out, "Profile name", "work", |a| {
        if a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            Ok(())
        } else {
            Err("letters, digits, - and _")
        }
    })?;
    Ok(JiraAnswers { name, base_url, email, token, board: board.parse().ok() })
}

fn not_empty(what: &'static str) -> impl Fn(&str) -> Result<(), &'static str> {
    move |a: &str| if a.is_empty() { Err(what) } else { Ok(()) }
}

fn token_question(input: &mut impl BufRead, out: &mut impl Write) -> std::io::Result<Token> {
    write!(
        out,
        "\nThe API token (Cloud: id.atlassian.com, Security, API tokens). kolum can\n  \
         1) run a command that prints it, like `pass show jira` (recommended)\n  \
         2) read $KOLUM_TOKEN\n  \
         3) keep it in config.toml, which is then readable by you alone\n"
    )?;
    loop {
        let how = ask_until(input, out, "Which", "1", |a| {
            if matches!(a, "1" | "2" | "3") { Ok(()) } else { Err("1, 2 or 3") }
        })?;
        match how.as_str() {
            "1" => {
                if let Some(cmd) = command_question(input, out)? {
                    return Ok(Token::Command(cmd));
                }
            }
            "2" => return Ok(Token::Environment),
            _ => {
                return ask_until(input, out, "Token", "", not_empty("the token"))
                    .map(Token::InFile);
            }
        }
    }
}

/// The command, or None when it runs no program: the choice is then asked
/// again.
fn command_question(
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> std::io::Result<Option<String>> {
    let cmd = ask_until(input, out, "Command", "", not_empty("a command"))?;
    if is_program(&cmd) {
        return Ok(Some(cmd));
    }
    // Not echoed back: it is most often the token itself.
    writeln!(out, "  not a program on your PATH: pick 3 to keep the token itself")?;
    Ok(None)
}

/// Whether the command's first word is a program `sh` can run: a token
/// pasted at the command prompt is not.
fn is_program(cmd: &str) -> bool {
    let Some(first) = cmd.split_whitespace().next() else { return false };
    if first.contains('/') {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let path = match (first.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => first.into(),
        };
        return path.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(first).is_file()))
}

/// The profile as TOML, ready to append to config.toml.
pub fn profile_toml(a: &JiraAnswers) -> String {
    let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut out = format!(
        "\n[profiles.{}]\nprovider = \"jira\"\nbase_url = {}\n",
        a.name,
        quote(&a.base_url)
    );
    if let Some(email) = &a.email {
        out += &format!("email = {}\n", quote(email));
    }
    match &a.token {
        Token::Command(cmd) => out += &format!("token_cmd = {}\n", quote(cmd)),
        Token::Environment => out += "# the token is read from $KOLUM_TOKEN\n",
        Token::InFile(token) => out += &format!("token = {}\n", quote(token)),
    }
    if let Some(board) = a.board {
        out += &format!("board = {board}\n");
    }
    out
}

/// Appends the profile. A file holding a token is made readable by its
/// owner alone, before the token is written to it.
pub fn write_profile(path: &Path, a: &JiraAnswers) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if matches!(a.token, Token::InFile(_)) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    file.write_all(profile_toml(a).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(answers: &str) -> (Answer, String) {
        let mut out = vec![];
        let answer = questions(&mut answers.as_bytes(), &mut out).unwrap();
        (answer, String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_cloud_profile_with_a_token_command() {
        let (answer, said) = run(
            "\nacme.atlassian.net\nhttps://acme.atlassian.net/\nme\nme@acme.com\n\ncat ~/.jira-token\n42\n\n",
        );
        assert!(said.contains("a URL, starting with https://"), "a wrong answer is asked again");
        assert!(said.contains("an e-mail address"));
        let Answer::Jira(a) = answer else { panic!() };
        assert_eq!(
            a,
            JiraAnswers {
                name: "work".into(),
                base_url: "https://acme.atlassian.net".into(),
                email: Some("me@acme.com".into()),
                token: Token::Command("cat ~/.jira-token".into()),
                board: Some(42),
            }
        );
        let toml = profile_toml(&a);
        let (config, problems) = crate::config::parse(&toml);
        assert!(problems.is_empty(), "{problems:?}");
        let profile = &config.profiles["work"];
        assert_eq!(profile.settings["token_cmd"].as_str(), Some("cat ~/.jira-token"));
        assert_eq!(profile.settings["board"].as_integer(), Some(42));
    }

    #[test]
    fn a_token_pasted_as_the_command_is_refused_without_being_shown() {
        let (answer, said) = run(
            "\nhttps://a.atlassian.net\nme@a.com\n\nATATT3xFfGF0secret\n3\nATATT3xFfGF0secret\n\n\n",
        );
        let Answer::Jira(a) = answer else { panic!() };
        assert_eq!(a.token, Token::InFile("ATATT3xFfGF0secret".into()));
        assert!(said.contains("not a program"), "{said}");
        assert!(!said.contains("ATATT"), "the token is never written back");
        for (cmd, ok) in
            [("/bin/sh -c x", true), ("~/no/such/kolum-cmd", false), ("ATATT/x+y=", false)]
        {
            assert_eq!(is_program(cmd), ok, "{cmd}");
        }
    }

    #[test]
    fn a_data_center_profile_needs_no_email_and_demo_needs_nothing() {
        let (answer, _) = run("jira\nhttps://jira.corp\n2\n\nperso\n");
        let Answer::Jira(a) = answer else { panic!() };
        assert_eq!(
            (a.email, a.token, a.board, a.name.as_str()),
            (None, Token::Environment, None, "perso")
        );
        assert_eq!(run("demo\n").0, Answer::Demo);
        assert!(questions(&mut "".as_bytes(), &mut vec![]).is_err(), "no answer at all stops");
    }

    #[test]
    fn a_token_in_the_file_makes_the_file_private() {
        let dir = std::env::temp_dir().join(format!("kolum-wizard-{}", std::process::id()));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\ntheme = \"gruvbox\"\n").unwrap();
        let (Answer::Jira(a), _) = run("\nhttps://a.atlassian.net\nme@a.com\n3\ns3cr\"t\n\n\n")
        else {
            panic!()
        };
        write_profile(&path, &a).unwrap();
        let (config, problems) = crate::config::load(&path);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.ui.theme, "gruvbox", "what was there stays");
        assert_eq!(config.profiles["work"].settings["token"].as_str(), Some("s3cr\"t"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
