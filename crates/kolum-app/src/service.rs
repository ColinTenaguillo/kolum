use crate::board::{BoardSnapshot, load_board};
use kolum_domain::{
    ActivityEntry, BoardId, Capabilities, Comment, DomainError, Issue, IssueKey, Provider, User,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// What the TUI asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `None` opens the provider's first board.
    LoadBoard(Option<BoardId>),
    LoadIssue(IssueKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueDetail {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub activity: Vec<ActivityEntry>,
}

/// What comes back, in whatever order the requests finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Me(User),
    BoardLoaded(BoardSnapshot),
    IssueLoaded(Box<IssueDetail>),
    /// `what` says which request failed, in words for a status line.
    Failed {
        what: String,
        error: DomainError,
    },
}

/// The TUI's handle on a provider. Every command runs on its own task, so a
/// slow request never holds up the next one, nor the screen.
pub struct Service {
    commands: mpsc::UnboundedSender<Command>,
    capabilities: Capabilities,
    provider: String,
}

impl Service {
    /// Starts the service on the current tokio runtime. Asks for the
    /// signed-in user straight away: the board marks that user's cards.
    pub fn spawn(provider: Arc<dyn Provider>) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (commands, mut rx) = mpsc::unbounded_channel();
        let (events, events_rx) = mpsc::unbounded_channel();
        let service = Service {
            commands,
            capabilities: provider.capabilities(),
            provider: provider.name().to_string(),
        };
        let me = (provider.clone(), events.clone());
        tokio::spawn(async move {
            let event = match me.0.me().await {
                Ok(user) => Event::Me(user),
                Err(error) => Event::Failed { what: "who am I".into(), error },
            };
            let _ = me.1.send(event);
        });
        tokio::spawn(async move {
            while let Some(command) = rx.recv().await {
                tokio::spawn(run(provider.clone(), command, events.clone()));
            }
        });
        (service, events_rx)
    }

    /// A send fails only once the service task is gone, which happens only
    /// at shutdown; there is nobody left to tell.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn provider_name(&self) -> &str {
        &self.provider
    }
}

async fn run(provider: Arc<dyn Provider>, command: Command, events: mpsc::UnboundedSender<Event>) {
    let event = match command {
        Command::LoadBoard(id) => match load_board(provider.as_ref(), id).await {
            Ok(snapshot) => Event::BoardLoaded(snapshot),
            Err(error) => Event::Failed { what: "loading the board".into(), error },
        },
        Command::LoadIssue(key) => match load_issue(provider.as_ref(), &key).await {
            Ok(detail) => Event::IssueLoaded(Box::new(detail)),
            Err(error) => Event::Failed { what: format!("loading {key}"), error },
        },
    };
    if let Event::Failed { what, error } = &event {
        tracing::warn!("{what}: {error}");
    }
    let _ = events.send(event);
}

async fn load_issue(provider: &dyn Provider, key: &IssueKey) -> kolum_domain::Result<IssueDetail> {
    let (issue, comments, activity) =
        tokio::try_join!(provider.issue(key), provider.comments(key), provider.activity(key))?;
    Ok(IssueDetail { issue, comments, activity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_provider_fake::FakeProvider;

    #[tokio::test]
    async fn commands_come_back_as_events() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()));
        assert_eq!(service.provider_name(), "fake");
        let Some(Event::Me(me)) = events.recv().await else { panic!("expected Me first") };
        assert_eq!(me.display_name, "Colin Tenaguillo");

        service.send(Command::LoadBoard(None));
        let Some(Event::BoardLoaded(snapshot)) = events.recv().await else { panic!() };
        assert_eq!(snapshot.board.columns.len(), 5);

        service.send(Command::LoadIssue(IssueKey::new("ACME-5")));
        let Some(Event::IssueLoaded(detail)) = events.recv().await else { panic!() };
        assert_eq!(detail.comments.len(), 3);
        assert!(!detail.activity.is_empty());
    }

    #[tokio::test]
    async fn a_failure_names_what_failed() {
        let (service, mut events) = Service::spawn(Arc::new(FakeProvider::new()));
        events.recv().await; // Me
        service.send(Command::LoadIssue(IssueKey::new("ACME-999")));
        let Some(Event::Failed { what, error }) = events.recv().await else { panic!() };
        assert_eq!(what, "loading ACME-999");
        assert!(matches!(error, DomainError::NotFound(_)));
    }
}
