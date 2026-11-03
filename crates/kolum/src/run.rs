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
        for effect in app.take_effects() {
            perform(effect, &service);
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
        // OSC 52: the terminal sets the clipboard, which also works over
        // ssh, where no clipboard tool on this side could reach it.
        Effect::Copy(text) => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(text);
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b]52;c;{encoded}\x07").and_then(|_| out.flush());
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
