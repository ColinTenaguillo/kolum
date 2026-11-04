//! An in-memory provider: a realistic board, writable, with no account.
//!
//! It backs `kolum --demo`, the test suite and development before the
//! author knows which Jira they have. Writes change the memory only, so a
//! demo can be clicked through and starts fresh next time.

mod data;

use async_trait::async_trait;
use kolum_domain::*;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use time::{Duration, OffsetDateTime, macros::datetime};

/// The board's issues come back in pages this long, so the pagination
/// loop in the application is exercised by every demo.
const PAGE_SIZE: usize = 20;
const BOARD_ID: &str = "1";

pub struct FakeProvider {
    state: Mutex<State>,
}

struct State {
    issues: Vec<Issue>,
    comments: HashMap<IssueKey, Vec<Comment>>,
    activity: HashMap<IssueKey, Vec<ActivityEntry>>,
}

fn user(i: usize) -> User {
    let (id, name) = data::USERS[i];
    User {
        id: UserId::new(id),
        display_name: name.into(),
        avatar_url: Some(format!("fake://avatar/{id}")),
    }
}

/// The demo's files: a screenshot and a log on the SSO bug.
fn attachments(n: u32) -> Vec<Attachment> {
    match n {
        5 => vec![
            Attachment {
                name: "session-expired.png".into(),
                url: "fake://shot/5".into(),
                mime: "image/png".into(),
                size: 48213,
            },
            Attachment {
                name: "callback.log".into(),
                url: "fake://file/5".into(),
                mime: "text/plain".into(),
                size: 1830,
            },
        ],
        _ => vec![],
    }
}

/// A 48x27 screenshot-like picture: a title bar and a dialog.
fn screenshot() -> Vec<u8> {
    let (w, h) = (48, 27);
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            let pixel = match (x, y) {
                (_, 0..3) => [60, 60, 70],
                (14..34, 9..18) => [230, 90, 90],
                _ => [235, 235, 240],
            };
            out.extend(pixel);
        }
    }
    out
}

/// A 16x16 avatar, as a PPM (the simplest format an image reader takes):
/// the user's colour with a lighter cross, so each looks like someone.
fn avatar(id: &str) -> Vec<u8> {
    let hash = id.bytes().fold(0x811c9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x01000193));
    let base = [(hash >> 16) as u8 | 0x40, (hash >> 8) as u8 | 0x40, hash as u8 | 0x40];
    let mut out = b"P6\n16 16\n255\n".to_vec();
    for y in 0..16 {
        for x in 0..16 {
            let light = (5..11).contains(&x) && (3..7).contains(&y)
                || (3..13).contains(&x) && (9..14).contains(&y);
            out.extend(base.map(|c| if light { c.saturating_add(90) } else { c }));
        }
    }
    out
}

fn status(id: &str) -> Status {
    let &(id, name, category) =
        data::STATUSES.iter().find(|s| s.0 == id).expect("data names a known status");
    let category = match category {
        0 => StatusCategory::Todo,
        1 => StatusCategory::Doing,
        _ => StatusCategory::Done,
    };
    Status { id: StatusId::new(id), name: name.into(), category }
}

fn key(n: u32) -> IssueKey {
    IssueKey::new(format!("{}-{n}", data::PROJECT_KEY))
}

fn type_name(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::Bug => "Bug",
        IssueKind::Story => "Story",
        IssueKind::Task => "Task",
        IssueKind::Epic => "Epic",
        IssueKind::Subtask => "Sub-task",
        IssueKind::Other => "Other",
    }
}

fn row_ref(n: u32) -> IssueRef {
    let row = data::ROWS.iter().find(|r| r.n == n).expect("data names a known issue");
    IssueRef {
        key: key(n),
        title: row.title.into(),
        kind: row.kind,
        status: Some(status(row.status)),
    }
}

/// Fixed dates, so a snapshot of the board renders the same every day.
fn created_at(n: u32) -> OffsetDateTime {
    datetime!(2026-09-01 09:00 UTC) + Duration::hours(7 * i64::from(n))
}

fn issue(row: &data::Row) -> Issue {
    let created = created_at(row.n);
    let links = match row.n {
        10 => vec![IssueLink { relation: "is blocked by".into(), issue: row_ref(34) }],
        34 => vec![IssueLink { relation: "blocks".into(), issue: row_ref(10) }],
        29 => vec![IssueLink { relation: "relates to".into(), issue: row_ref(13) }],
        _ => vec![],
    };
    let in_sprint = !matches!(row.status, "backlog") && row.kind != IssueKind::Epic;
    Issue {
        id: IssueId::new(format!("100{:02}", row.n)),
        key: key(row.n),
        title: row.title.into(),
        kind: IssueType { name: type_name(row.kind).into(), kind: row.kind },
        status: status(row.status),
        priority: Some(row.priority),
        assignee: row.assignee.map(user),
        reporter: Some(user(row.reporter)),
        labels: row.labels.iter().map(|l| l.to_string()).collect(),
        parent: row.epic.map(row_ref),
        subtasks: data::ROWS
            .iter()
            .filter(|r| row.kind == IssueKind::Epic && r.epic == Some(row.n))
            .map(|r| row_ref(r.n))
            .collect(),
        links,
        estimate: row.points.map(Estimate::Points),
        sprint: in_sprint.then(|| Sprint { id: "14".into(), name: "Sprint 14".into() }),
        created,
        updated: created + Duration::hours(3 * i64::from(row.n % 9)),
        due: (row.priority == Priority::Highest).then(|| (created + Duration::days(10)).date()),
        description: RichText::from_markdown(row.description),
        fields: vec![CustomField {
            id: FieldId::new("customfield_10020"),
            name: "Team".into(),
            value: FieldValue::Text(
                if row.labels.contains(&"mobile") { "Mobile" } else { "Platform" }.into(),
            ),
        }],
        url: Some(format!("https://demo.kolum.invalid/browse/{}", key(row.n))),
        attachments: attachments(row.n),
    }
}

/// Created by its reporter, and moved by its assignee (or the reporter)
/// unless it is still in the backlog.
fn history(issue: &Issue, reporter: User) -> Vec<ActivityEntry> {
    let mut entries = vec![ActivityEntry {
        author: reporter.clone(),
        at: issue.created,
        changes: vec![FieldChange {
            field: "Status".into(),
            from: None,
            to: Some("Backlog".into()),
        }],
    }];
    if issue.status.id.as_str() != "backlog" {
        entries.push(ActivityEntry {
            author: issue.assignee.clone().unwrap_or(reporter),
            at: issue.updated,
            changes: vec![FieldChange {
                field: "Status".into(),
                from: Some("Backlog".into()),
                to: Some(issue.status.name.clone()),
            }],
        });
    }
    entries
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeProvider {
    pub fn new() -> Self {
        let issues: Vec<Issue> = data::ROWS.iter().map(issue).collect();
        let mut comments: HashMap<IssueKey, Vec<Comment>> = HashMap::new();
        for (i, &(n, author, body)) in data::COMMENTS.iter().enumerate() {
            comments.entry(key(n)).or_default().push(Comment {
                id: format!("c{i}"),
                author: user(author),
                body: RichText::from_markdown(body),
                created: created_at(n) + Duration::hours(i as i64 + 1),
            });
        }
        let activity = issues
            .iter()
            .zip(data::ROWS)
            .map(|(i, row)| (i.key.clone(), history(i, user(row.reporter))))
            .collect();
        Self { state: Mutex::new(State { issues, comments, activity }) }
    }

    /// A poisoned lock means a test panicked mid-write; the data is still
    /// whole, since every write is a single assignment.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes one issue, and stamps it as updated: the incremental sync
    /// finds changes by that stamp.
    fn with_issue<T>(&self, key: &IssueKey, f: impl FnOnce(&mut Issue) -> T) -> Result<T> {
        let mut state = self.state();
        let issue = state
            .issues
            .iter_mut()
            .find(|i| &i.key == key)
            .ok_or_else(|| DomainError::NotFound(key.to_string()))?;
        issue.updated = OffsetDateTime::now_utc();
        Ok(f(issue))
    }
}

fn board() -> Board {
    Board {
        id: BoardId::new(BOARD_ID),
        name: format!("{} board", data::PROJECT_NAME),
        kind: BoardKind::Kanban,
        project: Some(project()),
        columns: data::COLUMNS
            .iter()
            .map(|&(name, statuses, wip_limit)| Column {
                name: name.into(),
                statuses: statuses.iter().map(|s| StatusId::new(*s)).collect(),
                wip_limit,
            })
            .collect(),
    }
}

fn project() -> Project {
    Project { key: data::PROJECT_KEY.into(), name: data::PROJECT_NAME.into() }
}

fn page<T: Clone>(items: Vec<T>, token: Option<PageToken>) -> Page<T> {
    let start: usize = token.and_then(|t| t.0.parse().ok()).unwrap_or(0);
    let end = (start + PAGE_SIZE).min(items.len());
    let next = (end < items.len()).then(|| PageToken(end.to_string()));
    Page { items: items.get(start..end).unwrap_or_default().to_vec(), next }
}

fn text_matches(issue: &Issue, text: &str) -> bool {
    let text = text.to_lowercase();
    [issue.key.as_str(), &issue.title, &issue.description.plain_text()]
        .iter()
        .any(|h| h.to_lowercase().contains(&text))
}

#[async_trait]
impl BoardSource for FakeProvider {
    async fn boards(&self) -> Result<Vec<BoardSummary>> {
        let b = board();
        Ok(vec![BoardSummary { id: b.id, name: b.name, kind: b.kind, project: b.project }])
    }

    async fn board(&self, id: &BoardId) -> Result<Board> {
        match id.as_str() {
            BOARD_ID => Ok(board()),
            _ => Err(DomainError::NotFound(format!("board {id}"))),
        }
    }

    async fn board_issues(
        &self,
        id: &BoardId,
        since: Option<OffsetDateTime>,
        token: Option<PageToken>,
    ) -> Result<Page<Issue>> {
        self.board(id).await?;
        // Epics live above the board, as on Jira: they are parents, not cards.
        let issues = self
            .state()
            .issues
            .iter()
            .filter(|i| i.kind.kind != IssueKind::Epic)
            .filter(|i| since.is_none_or(|s| i.updated >= s))
            .cloned()
            .collect();
        Ok(page(issues, token))
    }
}

#[async_trait]
impl IssueReader for FakeProvider {
    async fn issue(&self, key: &IssueKey) -> Result<Issue> {
        let state = self.state();
        let found = state.issues.iter().find(|i| &i.key == key).cloned();
        found.ok_or_else(|| DomainError::NotFound(key.to_string()))
    }

    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>> {
        Ok(self.state().comments.get(key).cloned().unwrap_or_default())
    }

    async fn activity(&self, key: &IssueKey) -> Result<Vec<ActivityEntry>> {
        Ok(self.state().activity.get(key).cloned().unwrap_or_default())
    }
}

#[async_trait]
impl IssueSearch for FakeProvider {
    async fn search(&self, query: &Query, token: Option<PageToken>) -> Result<Page<Issue>> {
        let Query::Filter(filter) = query else {
            return Err(DomainError::Unsupported(Capability::NativeQuery));
        };
        let me = UserId::new(data::USERS[0].0);
        let hits = self
            .state()
            .issues
            .iter()
            .filter(|i| filter.matches_fields(i, &me))
            .filter(|i| filter.text.as_ref().is_none_or(|t| text_matches(i, t)))
            .cloned()
            .collect();
        Ok(page(hits, token))
    }
}

#[async_trait]
impl IssueWriter for FakeProvider {
    /// Every status is one step from every other: the demo shows the
    /// mechanics, not a workflow.
    async fn transitions(&self, key: &IssueKey) -> Result<Vec<Transition>> {
        let current = self.issue(key).await?.status.id;
        Ok(data::STATUSES
            .iter()
            .filter(|s| s.0 != current.as_str())
            .map(|s| Transition {
                id: TransitionId::new(format!("to-{}", s.0)),
                name: s.1.into(),
                to: status(s.0),
                required_fields: vec![],
            })
            .collect())
    }

    async fn transition(&self, key: &IssueKey, transition: &TransitionId) -> Result<()> {
        let target = transition
            .as_str()
            .strip_prefix("to-")
            .filter(|t| data::STATUSES.iter().any(|s| s.0 == *t))
            .ok_or_else(|| DomainError::NotFound(format!("transition {transition}")))?;
        let (from, to) = self.with_issue(key, |i| {
            let from = std::mem::replace(&mut i.status, status(target));
            (from.name, i.status.name.clone())
        })?;
        self.state().activity.entry(key.clone()).or_default().push(ActivityEntry {
            author: user(0),
            at: OffsetDateTime::now_utc(),
            changes: vec![FieldChange { field: "Status".into(), from: Some(from), to: Some(to) }],
        });
        Ok(())
    }

    async fn assign(&self, key: &IssueKey, who: Option<&UserId>) -> Result<()> {
        let assignee = match who {
            None => None,
            Some(id) => Some(
                (0..data::USERS.len())
                    .map(user)
                    .find(|u| &u.id == id)
                    .ok_or_else(|| DomainError::NotFound(format!("user {id}")))?,
            ),
        };
        self.with_issue(key, |i| i.assignee = assignee)
    }

    async fn comment(&self, key: &IssueKey, body: RichText) -> Result<Comment> {
        self.issue(key).await?;
        let mut state = self.state();
        let list = state.comments.entry(key.clone()).or_default();
        let comment = Comment {
            id: format!("{key}-c{}", list.len()),
            author: user(0),
            body,
            created: OffsetDateTime::now_utc(),
        };
        list.push(comment.clone());
        Ok(comment)
    }

    async fn update(&self, key: &IssueKey, update: IssueUpdate) -> Result<()> {
        self.with_issue(key, |i| {
            if let Some(t) = update.title {
                i.title = t;
            }
            if let Some(d) = update.description {
                i.description = d;
            }
            if let Some(l) = update.labels {
                i.labels = l;
            }
            if update.priority.is_some() {
                i.priority = update.priority;
            }
        })
    }

    async fn create(&self, new: NewIssue) -> Result<Issue> {
        let mut state = self.state();
        let n = state.issues.len() as u32 + 1;
        let now = OffsetDateTime::now_utc();
        let mut issue = issue(&data::ROWS[0]);
        issue.id = IssueId::new(format!("200{n}"));
        issue.key = IssueKey::new(format!("{}-{n}", new.project));
        issue.title = new.title;
        issue.kind = new.kind;
        issue.description = new.description;
        issue.status = status("backlog");
        issue.parent = new.parent.map(|k| IssueRef {
            key: k,
            title: String::new(),
            kind: IssueKind::Epic,
            status: None,
        });
        (issue.assignee, issue.labels, issue.subtasks, issue.estimate, issue.sprint) =
            (None, vec![], vec![], None, None);
        (issue.created, issue.updated, issue.reporter) = (now, now, Some(user(0)));
        state.issues.push(issue.clone());
        Ok(issue)
    }

    /// The board's order is the order of the list: the issue is taken out
    /// and put back next to the other.
    async fn rank(&self, key: &IssueKey, rank: &Rank) -> Result<()> {
        let mut state = self.state();
        let (Rank::Before(other) | Rank::After(other)) = rank;
        let from = state.issues.iter().position(|i| &i.key == key);
        let from = from.ok_or_else(|| DomainError::NotFound(key.to_string()))?;
        if !state.issues.iter().any(|i| &i.key == other) {
            return Err(DomainError::NotFound(other.to_string()));
        }
        let issue = state.issues.remove(from);
        let at = state.issues.iter().position(|i| &i.key == other).unwrap_or(0);
        let at = if matches!(rank, Rank::After(_)) { at + 1 } else { at };
        state.issues.insert(at, issue);
        Ok(())
    }

    async fn clone_issue(&self, key: &IssueKey) -> Result<Issue> {
        let source = self.issue(key).await?;
        let new = NewIssue {
            project: data::PROJECT_KEY.into(),
            kind: source.kind,
            title: format!("CLONE - {}", source.title),
            description: source.description,
            parent: source.parent.map(|p| p.key),
        };
        self.create(new).await
    }
}

#[async_trait]
impl UserDirectory for FakeProvider {
    async fn me(&self) -> Result<User> {
        Ok(user(0))
    }

    async fn find_users(&self, query: &str) -> Result<Vec<User>> {
        let q = query.to_lowercase();
        Ok((0..data::USERS.len())
            .map(user)
            .filter(|u| u.display_name.to_lowercase().contains(&q))
            .collect())
    }

    async fn image(&self, url: &str) -> Result<Vec<u8>> {
        match url.strip_prefix("fake://avatar/") {
            Some(id) if data::USERS.iter().any(|u| u.0 == id) => Ok(avatar(id)),
            _ if url == "fake://shot/5" => Ok(screenshot()),
            _ => Err(DomainError::NotFound(url.into())),
        }
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> &str {
        "fake"
    }

    fn capabilities(&self) -> Capabilities {
        use Capability::*;
        Capabilities::new([
            Sprints,
            Epics,
            Estimates,
            WipLimits,
            CustomFields,
            Comments,
            Activity,
            Write,
            Images,
            Rank,
        ])
    }
}
