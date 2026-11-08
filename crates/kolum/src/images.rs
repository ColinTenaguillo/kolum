//! How this terminal can draw a picture, found out once, before raw mode:
//! the query writes an escape to stdout and reads the reply from stdin,
//! which the alternate screen would swallow.
//!
//! The query is only safe where the terminal answers it. One that answers
//! nothing (a pipe, `script`, tmux without allow-passthrough, a bridge that
//! drops the reply) leaves the library's reader thread on stdin for the
//! rest of the run, taking the keys meant for kolum. So a child process
//! asks first, and takes that thread with it when it exits; kolum asks
//! itself only once the child was answered.

use crate::config::ImagesMode;
use ratatui_image::picker::Picker;
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn probe(mode: ImagesMode) -> Option<Picker> {
    // An unknown terminal is not asked at all: the child's wait would be a
    // two-second pause at every start for nothing.
    let ask = || known_to_answer(|k| std::env::var(k).ok()).then(asked).flatten();
    picker(mode, ask)
}

/// What the terminal answered through the child; the log says when it
/// answered nothing worth asking for.
fn asked() -> Option<Picker> {
    let picker = query_through_child();
    if picker.is_none() {
        tracing::info!("no image protocol: avatars as half blocks");
    }
    picker
}

/// None, half blocks, or what `ask` found the terminal draws with; half
/// blocks when it found nothing.
fn picker(mode: ImagesMode, ask: impl FnOnce() -> Option<Picker>) -> Option<Picker> {
    match mode {
        ImagesMode::Off => None,
        ImagesMode::Halfblocks => Some(Picker::halfblocks()),
        ImagesMode::Auto => Some(ask().unwrap_or_else(Picker::halfblocks)),
    }
}

/// Terminals that say who they are and draw pictures. An environment can
/// lie (inherited by a pty that answers nothing), which the child covers.
pub fn known_to_answer(env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if set("TMUX") && !tmux_passes_through() {
        return false;
    }
    let term = env("TERM").unwrap_or_default();
    let program = env("TERM_PROGRAM").unwrap_or_default();
    set("KITTY_WINDOW_ID")
        || set("WEZTERM_EXECUTABLE")
        || set("GHOSTTY_RESOURCES_DIR")
        // Upstream answers without a protocol; forks with sixel name it.
        || set("ALACRITTY_WINDOW_ID")
        || ["kitty", "ghostty", "wezterm", "foot"].iter().any(|t| term.contains(t))
        || ["iTerm.app", "WezTerm", "ghostty"].contains(&program.as_str())
}

fn tmux_passes_through() -> bool {
    let out =
        std::process::Command::new("tmux").args(["show", "-gv", "allow-passthrough"]).output();
    out.is_ok_and(|o| matches!(String::from_utf8_lossy(&o.stdout).trim(), "on" | "all"))
}

/// The argument that makes this executable the probe below.
pub const PROBE_FLAG: &str = "--probe-images";

/// The child: query the terminal, say what it answered on stderr, exit.
pub fn probe_child() -> ! {
    match Picker::from_query_stdio() {
        Ok(p) => eprintln!("{:?}", p.protocol_type()),
        Err(_) => eprintln!("none"),
    }
    std::process::exit(0)
}

/// The library waits 2 s for an answer. A child done well before that was
/// answered; one past it is stuck, and is killed.
const ANSWERED_WITHIN: Duration = Duration::from_millis(1500);
const CHILD_DEADLINE: Duration = Duration::from_millis(2500);

fn query_through_child() -> Option<Picker> {
    // stdin and stdout stay the terminal's: the query must reach it.
    let mut probe = Command::new(std::env::current_exe().ok()?);
    let (said, took) = run_until(probe.arg(PROBE_FLAG), CHILD_DEADLINE)?;
    worth_querying(&said, took).then(query_here).flatten()
}

fn query_here() -> Option<Picker> {
    let picker = Picker::from_query_stdio().ok()?;
    tracing::info!("terminal image protocol: {:?}", picker.protocol_type());
    Some(picker)
}

/// What the command said on stderr, and how long it took; none when it
/// did not start, or ran past `deadline` and was killed.
fn run_until(command: &mut Command, deadline: Duration) -> Option<(String, Duration)> {
    let mut child = command.stderr(Stdio::piped()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::info!("the image probe did not finish: no answer from the terminal");
                return None;
            }
        }
    }
    let mut said = String::new();
    std::io::Read::read_to_string(&mut child.stderr?, &mut said).ok()?;
    Some((said, start.elapsed()))
}

/// A protocol better than half blocks, from a terminal that answered
/// before the timeout: a late "iTerm2" is only a guess from the
/// environment.
pub fn worth_querying(said: &str, took: Duration) -> bool {
    took < ANSWERED_WITHIN && matches!(said.trim(), "Kitty" | "Sixel" | "Iterm2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::ProtocolType;

    fn with(vars: &[(&str, &str)]) -> bool {
        known_to_answer(|k| vars.iter().find(|(name, _)| *name == k).map(|(_, v)| v.to_string()))
    }

    #[test]
    fn only_terminals_that_say_who_they_are_are_asked() {
        assert!(with(&[("KITTY_WINDOW_ID", "1")]));
        assert!(with(&[("TERM", "xterm-ghostty")]));
        assert!(with(&[("TERM_PROGRAM", "iTerm.app")]));
        assert!(with(&[("TERM", "foot")]));
        assert!(with(&[("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")]));
        // Alacritty keeps TERM as xterm's; a build with sixel says so when asked.
        assert!(with(&[("TERM", "xterm-256color"), ("ALACRITTY_WINDOW_ID", "94371842")]));
        assert!(!with(&[("TERM", "xterm-256color")]), "unknown: half blocks, no query");
        assert!(!with(&[("TERM_PROGRAM", "Apple_Terminal")]));
        assert!(!with(&[]));
        assert!(!with(&[("KITTY_WINDOW_ID", "")]), "set but empty is not set");
    }

    #[test]
    fn only_a_quick_answer_with_a_protocol_is_worth_a_query() {
        let quick = Duration::from_millis(80);
        let slow = Duration::from_millis(2100);
        assert!(worth_querying("Kitty\n", quick));
        assert!(worth_querying("Sixel", quick));
        assert!(!worth_querying("Iterm2", slow), "after the timeout: a guess from the environment");
        assert!(!worth_querying("Halfblocks", quick), "nothing better to ask for");
        assert!(!worth_querying("none", quick));
        assert!(!worth_querying("Kitty", ANSWERED_WITHIN), "at the limit is too late");
    }

    #[test]
    fn off_and_halfblocks_need_no_query() {
        assert!(probe(ImagesMode::Off).is_none());
        assert!(probe(ImagesMode::Halfblocks).is_some());
    }

    #[test]
    fn auto_draws_with_what_the_terminal_answered_else_half_blocks() {
        let mut kitty = Picker::halfblocks();
        kitty.set_protocol_type(ProtocolType::Kitty);
        let answered = picker(ImagesMode::Auto, || Some(kitty)).unwrap();
        assert_eq!(answered.protocol_type(), ProtocolType::Kitty);
        let silent = picker(ImagesMode::Auto, || None).unwrap();
        assert_eq!(silent.protocol_type(), ProtocolType::Halfblocks);
        assert!(picker(ImagesMode::Off, || unreachable!()).is_none(), "off asks nothing");
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_child_is_heard_out_or_killed_at_the_deadline() {
        let mut says = Command::new("sh");
        let (said, took) = run_until(says.args(["-c", "echo Kitty >&2"]), CHILD_DEADLINE).unwrap();
        assert_eq!((said.as_str(), took < CHILD_DEADLINE), ("Kitty\n", true));
        let mut stuck = Command::new("sleep");
        assert_eq!(run_until(stuck.arg("5"), Duration::from_millis(100)), None);
        assert_eq!(run_until(&mut Command::new("/no/such/kolum-probe"), CHILD_DEADLINE), None);
    }
}
