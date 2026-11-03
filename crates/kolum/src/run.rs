//! The event loop: keys, service events, the matcher's wake-ups and the
//! key-sequence deadline, whichever comes first. Nothing here blocks: the
//! network is the service's, the sorting nucleo's.

use crate::ui::{App, Effect};
use base64::Engine;
use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use kolum_app::{Event, Service};
use ratatui::DefaultTerminal;
use std::io::Write;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

pub async fn run(
    terminal: &mut DefaultTerminal,
    mut app: App,
    service: Service,
    mut events: UnboundedReceiver<Event>,
    mut wake: UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let mut keys = EventStream::new();
    loop {
        // Until none are left: what an edit sends must go now, not after
        // the next key.
        loop {
            let effects = app.take_effects();
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                let Effect::Edit { purpose, text } = effect else {
                    perform(effect, &service);
                    continue;
                };
                // The editor needs the terminal and stdin to itself:
                // crossterm's reader would race it for every key.
                drop(keys);
                ratatui::restore();
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| "vi".into());
                let result = tokio::task::spawn_blocking(move || edit_text(&text, &editor))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                *terminal = ratatui::init();
                keys = EventStream::new();
                app.on_edited(purpose, result);
            }
        }
        if app.quit {
            return Ok(());
        }
        app.tick();
        terminal.draw(|frame| app.draw(frame))?;
        let deadline = app.deadline().map(tokio::time::Instant::from_std);
        tokio::select! {
            key = keys.next() => match key {
                Some(Ok(TermEvent::Key(k))) if k.kind != KeyEventKind::Release => {
                    app.on_key(k.into(), Instant::now())
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
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

fn perform(effect: Effect, service: &Service) {
    match effect {
        Effect::Send(command) => service.send(command),
        // Handled by the loop, which owns the terminal.
        Effect::Edit { .. } => {}
        Effect::Copy(text) => {
            let mut out = std::io::stdout();
            let _ = out.write_all(osc52(&text).as_bytes()).and_then(|_| out.flush());
        }
        Effect::Open(url) => {
            let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
            let spawned = std::process::Command::new(opener)
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Err(e) = spawned {
                tracing::warn!("{opener} {url}: {e}");
            }
        }
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
    #[test]
    fn the_editor_edits_a_scratch_file_that_is_then_removed() {
        assert_eq!(super::edit_text("hello", "printf ' world' >>").unwrap(), "hello world");
        assert!(super::edit_text("x", "false").unwrap_err().contains("exited"));
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
        assert_eq!(super::osc52("ACME-5"), "\x1b]52;c;QUNNRS01\x07");
    }
}
