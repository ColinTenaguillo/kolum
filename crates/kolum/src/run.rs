//! The event loop: keys, service events, the matcher's wake-ups and the
//! key-sequence deadline, whichever comes first. Nothing here blocks: the
//! network is the service's, the sorting nucleo's.

use crate::ui::{App, EditPurpose, Effect};
use base64::Engine;
use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use kolum_app::{Event, Service};
use ratatui::DefaultTerminal;
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

/// How long quitting waits for what was read to be kept.
const FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

/// The loop until the user quits; then what was read is kept, for a while.
pub async fn run(
    state_file: Option<std::path::PathBuf>,
    terminal: &mut DefaultTerminal,
    app: App,
    service: Service,
    events: UnboundedReceiver<Event>,
    wake: UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    bracketed_paste(true);
    let result = run_loop(state_file, terminal, app, &service, events, wake).await;
    bracketed_paste(false);
    service.close(FLUSH).await;
    result
}

async fn run_loop(
    state_file: Option<std::path::PathBuf>,
    terminal: &mut DefaultTerminal,
    mut app: App,
    service: &Service,
    mut events: UnboundedReceiver<Event>,
    mut wake: UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let mut keys = EventStream::new();
    let mut queue = VecDeque::new();
    let mut out = std::io::stdout();
    loop {
        // Until none are left: what an edit sends must go now, not after
        // the next key.
        loop {
            let fresh = app.take_effects();
            let state_file = state_file.as_deref();
            let Some((purpose, text)) = drain(&mut queue, fresh, service, state_file, &mut out)
            else {
                break;
            };
            let result;
            (keys, result) = in_editor(terminal, keys, text).await;
            app.on_edited(purpose, result);
        }
        if app.quit {
            return Ok(());
        }
        app.tick();
        terminal.draw(|frame| app.draw(frame))?;
        let deadline = app.deadline().map(tokio::time::Instant::from_std);
        tokio::select! {
            key = keys.next() => if !on_term(&mut app, key)? {
                return Ok(());
            },
            Some(event) = events.recv() => app.on_event(event),
            Some(()) = wake.recv() => {
                // The matcher wakes once per batch; one redraw covers all.
                while wake.try_recv().is_ok() {}
            }
            _ = sleep_until(deadline), if deadline.is_some() => app.on_timeout(Instant::now()),
        }
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    if let Some(d) = deadline {
        tokio::time::sleep_until(d).await;
    }
}

/// What a terminal event is to the app: a key pressed or held goes in; a
/// release, a resize or the mouse do not (the next draw takes the new
/// size). False once the terminal has nothing more to say.
fn on_term(app: &mut App, event: Option<std::io::Result<TermEvent>>) -> std::io::Result<bool> {
    match event {
        Some(Ok(TermEvent::Key(k))) if k.kind != KeyEventKind::Release => {
            app.on_key(k.into(), Instant::now())
        }
        Some(Ok(TermEvent::Paste(text))) => app.on_paste(&text, Instant::now()),
        Some(Ok(_)) => {}
        Some(Err(e)) => return Err(e),
        None => return Ok(false),
    }
    Ok(true)
}

/// The effects `queue`d, then the `fresh` ones behind them, done in order
/// up to an edit: it needs the terminal, so it is handed back, what follows
/// it waiting in the queue.
fn drain(
    queue: &mut VecDeque<Effect>,
    fresh: Vec<Effect>,
    service: &Service,
    state_file: Option<&Path>,
    out: &mut impl Write,
) -> Option<(EditPurpose, String)> {
    queue.extend(fresh);
    std::iter::from_fn(|| queue.pop_front()).find_map(|e| perform(e, service, state_file, out))
}

/// One effect done; an edit is handed back, for the loop, which owns the
/// terminal.
fn perform(
    effect: Effect,
    service: &Service,
    state_file: Option<&Path>,
    out: &mut impl Write,
) -> Option<(EditPurpose, String)> {
    match effect {
        Effect::Send(command) => service.send(command),
        Effect::Edit { purpose, text } => return Some((purpose, text)),
        Effect::SaveState(state) => save_state(&state, state_file),
        Effect::Copy(text) => copy(&text, out),
        Effect::Open(url) => open(&url),
    }
    None
}

/// Kept for the next run, when there is a file to keep it in.
fn save_state(state: &crate::config::UiState, state_file: Option<&Path>) {
    if let Some(path) = state_file
        && let Err(e) = state.save(path)
    {
        tracing::warn!("saving {}: {e}", path.display());
    }
}

fn copy(text: &str, out: &mut impl Write) {
    let _ = out.write_all(osc52(text).as_bytes()).and_then(|_| out.flush());
}

/// What opens a URL in the browser here.
const OPENER: &str = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };

/// In the browser, through the system's opener.
fn open(url: &str) {
    let spawned = std::process::Command::new(OPENER)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        tracing::warn!("{OPENER} {url}: {e}");
    }
}

/// `$VISUAL`, `$EDITOR` or vi on `text`, with the terminal and stdin to
/// itself: crossterm's reader, `keys`, would race it for every key. The
/// result, and a reader started again.
async fn in_editor(
    terminal: &mut DefaultTerminal,
    keys: EventStream,
    text: String,
) -> (EventStream, Result<String, String>) {
    drop(keys);
    bracketed_paste(false);
    ratatui::restore();
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let result = tokio::task::spawn_blocking(move || edit_text(&text, &editor))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    *terminal = ratatui::init();
    bracketed_paste(true);
    (EventStream::new(), result)
}

/// A paste comes as one event, its text, rather than as keys, which the
/// bindings would run. Off for `$EDITOR`, and when kolum ends.
pub fn bracketed_paste(on: bool) {
    let _ = paste_mode(&mut std::io::stdout(), on)
        .inspect_err(|e| tracing::warn!("bracketed paste: {e}"));
}

fn paste_mode(out: &mut impl Write, on: bool) -> std::io::Result<()> {
    use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
    if on {
        crossterm::execute!(out, EnableBracketedPaste)
    } else {
        crossterm::execute!(out, DisableBracketedPaste)
    }
}

/// Writes `text` to a scratch Markdown file, runs `editor` on it through
/// the shell (so `EDITOR="code -w"` works), and reads it back.
fn edit_text(text: &str, editor: &str) -> Result<String, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("kolum-{}-{stamp}.md", std::process::id()));
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("kolum")
        .arg(&path)
        .status();
    let result = match status {
        Ok(s) if s.success() => std::fs::read_to_string(&path).map_err(|e| e.to_string()),
        Ok(s) => Err(format!("`{editor}` exited with {s}")),
        Err(e) => Err(format!("`{editor}`: {e}")),
    };
    let _ = std::fs::remove_file(&path);
    result
}

/// OSC 52: the terminal sets the clipboard, which also works over ssh,
/// where no clipboard tool on this side could reach it.
fn osc52(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UiState;
    use crate::ui::Settings;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use kolum_app::Command;
    use kolum_domain::IssueKey;
    use std::sync::Arc;

    #[test]
    fn a_key_pressed_goes_to_the_app_a_release_does_not_and_the_end_ends_the_loop() {
        let q = |kind| {
            let key = KeyEvent::new_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, kind);
            Some(Ok(TermEvent::Key(key)))
        };
        let mut app = App::new(Settings::default(), Arc::new(|| {}));
        assert!(on_term(&mut app, q(KeyEventKind::Release)).unwrap());
        assert!(on_term(&mut app, Some(Ok(TermEvent::Resize(80, 24)))).unwrap());
        assert!(!app.quit, "a release, a resize: no key");
        assert!(on_term(&mut app, q(KeyEventKind::Press)).unwrap());
        assert!(app.quit, "q on the bare board quits");
        assert!(!on_term(&mut app, None).unwrap(), "the terminal has no more");
        assert!(on_term(&mut app, Some(Err(std::io::Error::other("gone")))).is_err());
    }

    #[test]
    fn bracketed_paste_is_the_terminals_mode_2004() {
        let written = |on| {
            let mut out = vec![];
            paste_mode(&mut out, on).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!([written(true), written(false)], ["\x1b[?2004h", "\x1b[?2004l"]);
    }

    /// A paste is text, not keys: `q` pasted on the board quits nothing,
    /// and a key opens its issue.
    #[test]
    fn a_paste_goes_to_the_app_as_text() {
        let mut app = App::new(Settings::default(), Arc::new(|| {}));
        app.take_effects();
        assert!(on_term(&mut app, Some(Ok(TermEvent::Paste("q".into())))).unwrap());
        assert!(!app.quit);
        on_term(&mut app, Some(Ok(TermEvent::Paste("ACME-5".into())))).unwrap();
        let open = Effect::Send(Command::LoadIssue(IssueKey::parse("ACME-5")));
        assert_eq!(app.take_effects(), [open]);
    }

    /// A directory of its own, removed with it, pass or fail.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kolum-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Each effect goes out in turn, those left from before first; an edit
    /// stops there, holding back what follows it.
    #[tokio::test]
    async fn effects_go_out_in_order_up_to_an_edit_which_holds_back_the_rest() {
        let (service, mut events) =
            Service::spawn(Arc::new(kolum_provider_fake::FakeProvider::new()), None);
        let scratch = Scratch::new("drain");
        let file = scratch.0.join("state.toml");
        let state = UiState {
            hide_empty_columns: true,
            swimlanes: Default::default(),
            applied_filter: Some("@me".into()),
        };
        let purpose = EditPurpose::Comment(IssueKey::parse("ACME-5"));
        let edit = Effect::Edit { purpose: purpose.clone(), text: "hi".into() };
        let boards = Effect::Send(Command::LoadBoards);
        let save = Effect::SaveState(state.clone());
        let mut queue = VecDeque::from([Effect::Copy("ACME-5".into()), edit, boards.clone()]);
        let mut out = vec![];
        let mut drain = |queue: &mut VecDeque<Effect>, fresh| {
            drain(queue, fresh, &service, Some(&file), &mut out)
        };
        assert_eq!(drain(&mut queue, vec![save.clone()]), Some((purpose, "hi".into())));
        assert_eq!(queue, [boards, save], "held back, the fresh one behind");
        assert_eq!(UiState::load(&file), None, "not yet kept");
        assert_eq!(drain(&mut queue, vec![]), None);
        assert!(queue.is_empty());
        assert_eq!(out, osc52("ACME-5").as_bytes(), "copied");
        assert_eq!(UiState::load(&file), Some(state), "kept");
        let listed = async {
            while let Some(event) = events.recv().await {
                if matches!(event, Event::Boards(_)) {
                    return true;
                }
            }
            false
        };
        let listed = tokio::time::timeout(std::time::Duration::from_secs(2), listed).await;
        assert_eq!(listed, Ok(true), "sent");
    }

    #[test]
    fn the_editor_edits_a_scratch_file_that_is_then_removed() {
        assert_eq!(edit_text("hello", "printf ' world' >>").unwrap(), "hello world");
        assert!(edit_text("x", "false").unwrap_err().contains("exited"));
        let leftovers =
            std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(Result::ok).filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("kolum-{}-", std::process::id()))
            });
        assert_eq!(leftovers.count(), 0);
    }

    #[test]
    fn osc52_carries_the_text_in_base64() {
        assert_eq!(osc52("ACME-5"), "\x1b]52;c;QUNNRS01\x07");
    }
}
