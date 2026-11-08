//! The GitHub Projects adapter, the second provider, there to prove the
//! domain is not Jira's in disguise.
//!
//! A ProjectV2 is a board. Its single-select Status field gives the
//! columns, "No Status" first as on github.com, and a transition sets that
//! field. Issues, pull requests and draft issues are its cards; their
//! bodies are Markdown, which RichText reads and writes as it is.

mod queries;
mod settings;

pub use settings::GithubSettings;

use async_trait::async_trait;
use kolum_domain::*;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use time::OffsetDateTime;
use tokio::sync::OnceCell;

/// The project as its fields describe it.
#[derive(Debug, Clone)]
struct ProjectInfo {
    id: String,
    title: String,
    owner: String,
    status_field: Option<String>,
    options: Vec<(String, String)>,
    /// Each option of the Priority field by id, ranked.
    priorities: HashMap<String, Priority>,
}

/// What a key stands for, to reach it again: the project item (for its
/// fields) and the issue, pull request or draft behind it.
#[derive(Debug, Clone)]
struct Known {
    item: Option<String>,
    content: String,
    kind: String,
    issue: Issue,
}

pub struct GithubProvider {
    http: reqwest::Client,
    settings: GithubSettings,
    project: OnceCell<ProjectInfo>,
    known: Mutex<HashMap<IssueKey, Known>>,
    /// The project's items in its order, from the last full read of the
    /// board: GitHub places an item after another, so "before X" is "after
    /// the one before X".
    order: Mutex<Vec<String>>,
}

fn transport(e: impl std::fmt::Display) -> DomainError {
    DomainError::Transport(e.to_string())
}

/// GraphQL answers 200 with `errors` for most failures; a partial answer
/// (one of `user` and `organization` not found) still has its data.
fn graphql_result(status: u16, body: &[u8]) -> Result<Value> {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if let Some(error) = http_error(status, parsed["message"].as_str().unwrap_or("")) {
        return Err(error);
    }
    let data = parsed["data"].clone();
    let has_data = data.as_object().is_some_and(|d| d.values().any(|v| !v.is_null()));
    match parsed["errors"].get(0) {
        Some(e) if !has_data => Err(graphql_error(e)),
        _ => Ok(data),
    }
}

fn http_error(status: u16, message: &str) -> Option<DomainError> {
    match status {
        200..=299 => None,
        401 => Some(DomainError::Unauthorized(message.to_string())),
        403 if message.to_lowercase().contains("rate limit") => {
            Some(DomainError::RateLimited { retry_after_secs: None })
        }
        _ => Some(DomainError::Transport(format!("HTTP {status}: {message}"))),
    }
}

/// The first of GraphQL's `errors`, by its type.
fn graphql_error(e: &Value) -> DomainError {
    let text = e["message"].as_str().unwrap_or("GraphQL error").to_string();
    match e["type"].as_str() {
        Some("NOT_FOUND") => DomainError::NotFound(text),
        Some("FORBIDDEN") | Some("INSUFFICIENT_SCOPES") => DomainError::Unauthorized(text),
        Some("RATE_LIMITED") => DomainError::RateLimited { retry_after_secs: None },
        _ => DomainError::Transport(text),
    }
}

fn datetime(v: &Value) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(v.as_str()?, &time::format_description::well_known::Rfc3339).ok()
}

fn user(v: &Value) -> Option<User> {
    let login = v["login"].as_str()?;
    Some(User {
        id: UserId::new(v["id"].as_str().unwrap_or(login)),
        display_name: v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(login).to_string(),
        avatar_url: v["avatarUrl"].as_str().map(String::from),
    })
}

/// GitHub's status names are free text; these words are what people use.
fn category(name: &str) -> StatusCategory {
    let name = name.to_lowercase();
    if ["done", "closed", "shipped", "complete", "merged"].iter().any(|w| name.contains(w)) {
        StatusCategory::Done
    } else if ["progress", "review", "doing", "active", "testing"].iter().any(|w| name.contains(w))
    {
        StatusCategory::Doing
    } else {
        StatusCategory::Todo
    }
}

/// Each option of the project's Priority field by id, ranked as the
/// domain ranks a provider's list: by its meaning, else by its place.
fn priority_ranks(fields: &[Value]) -> HashMap<String, Priority> {
    let field = fields.iter().find(|f| {
        f["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("priority"))
            && f["options"].is_array()
    });
    let options =
        field.and_then(|f| f["options"].as_array()).map(Vec::as_slice).unwrap_or_default();
    let names: Vec<&str> = options.iter().map(|o| o["name"].as_str().unwrap_or("")).collect();
    let ids = options.iter().map(|o| o["id"].as_str().unwrap_or("").to_string());
    ids.zip(Priority::list(&names)).collect()
}

/// A priority by its option, as the project's field ranks it; an option
/// the project did not list when read (added since) by its name.
fn priority(ranks: &HashMap<String, Priority>, option: &str, name: &str) -> Priority {
    ranks.get(option).cloned().unwrap_or_else(|| unplaced(name))
}

/// A priority with no place to rank it by: by what its name means, else
/// the middle, said once per name that means nothing.
fn unplaced(name: &str) -> Priority {
    static SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if Priority::known_level(name).is_none() && said.insert(name.to_string()) {
        tracing::info!("priority {name}: not among the project's options, ranked in the middle");
    }
    Priority::ranked(name, None)
}

const ESTIMATE_FIELDS: &[&str] = &["estimate", "size", "points", "story points"];

fn no_status() -> Status {
    Status { id: StatusId::new(""), name: "No Status".into(), category: StatusCategory::Todo }
}

/// GitHub's own key, `repo#12`, of an issue or a pull request: numbered
/// in its repository. A draft has neither.
fn numbered(content: &Value) -> Option<IssueKey> {
    let repo = content["repository"]["name"].as_str()?;
    Some(IssueKey::new(repo, "#", content["number"].as_u64()?))
}

/// A draft's key, in no repository: `draft-` and the end of its id.
fn draft_key(id: &str) -> IssueKey {
    let start = id.char_indices().rev().nth(5).map_or(0, |(at, _)| at);
    IssueKey::parse(format!("draft-{}", &id[start..]))
}

/// What a project item's field values say beyond its content: the
/// Status and Priority options, the estimate, the iteration, and the
/// other fields by name.
#[derive(Default)]
struct ItemFields {
    status: Option<Status>,
    priority: Option<Priority>,
    estimate: Option<Estimate>,
    sprint: Option<Sprint>,
    custom: Vec<CustomField>,
}

impl ItemFields {
    fn read(fields: &[Value], status_field: &str, ranks: &HashMap<String, Priority>) -> Self {
        let mut read = ItemFields::default();
        for v in fields {
            let name = v["field"]["name"].as_str().unwrap_or("");
            if let (Some(option), Some(value)) = (v["optionId"].as_str(), v["name"].as_str()) {
                read.option(name, option, value, status_field, ranks);
            } else if let Some(n) = v["number"].as_f64() {
                read.number(name, n);
            } else if let (Some(id), Some(title)) = (v["iterationId"].as_str(), v["title"].as_str())
            {
                read.sprint = Some(Sprint { id: id.into(), name: title.into() });
            }
        }
        read
    }

    fn option(
        &mut self,
        name: &str,
        option: &str,
        value: &str,
        status_field: &str,
        ranks: &HashMap<String, Priority>,
    ) {
        if name == status_field {
            let category = category(value);
            self.status = Some(Status { id: StatusId::new(option), name: value.into(), category });
        } else if name.eq_ignore_ascii_case("priority") {
            self.priority = Some(priority(ranks, option, value));
        } else {
            self.custom.push(custom(name, FieldValue::Text(value.into())));
        }
    }

    fn number(&mut self, name: &str, n: f64) {
        if ESTIMATE_FIELDS.contains(&name.to_lowercase().as_str()) {
            self.estimate = Some(Estimate::Points(n));
        } else {
            self.custom.push(custom(name, FieldValue::Number(n)));
        }
    }
}

fn custom(name: &str, value: FieldValue) -> CustomField {
    CustomField { id: FieldId::new(name), name: name.into(), value }
}

/// A pull request, a draft, an issue labelled as a bug, or an issue.
fn issue_type(kind: &str, labels: &[String]) -> IssueType {
    match kind {
        "PullRequest" => IssueType { name: "Pull request".into(), kind: IssueKind::Other },
        "DraftIssue" => IssueType { name: "Draft".into(), kind: IssueKind::Task },
        _ if labels.iter().any(|l| l.to_lowercase().contains("bug")) => {
            IssueType { name: "Bug".into(), kind: IssueKind::Bug }
        }
        _ => IssueType { name: "Issue".into(), kind: IssueKind::Task },
    }
}

/// A search hit's status, from open or closed; an item's with no Status
/// value is "No Status".
fn content_status(content: &Value, item: Option<&str>) -> Status {
    let status = |id: &str, name: &str, category| Status {
        id: StatusId::new(id),
        name: name.into(),
        category,
    };
    match content["state"].as_str() {
        Some("CLOSED" | "MERGED") if item.is_none() => {
            status("closed", "Closed", StatusCategory::Done)
        }
        Some(_) if item.is_none() => status("open", "Open", StatusCategory::Todo),
        _ => no_status(),
    }
}

fn labels_of(content: &Value) -> Vec<String> {
    let nodes = content["labels"]["nodes"].as_array().into_iter().flatten();
    nodes.filter_map(|l| l["name"].as_str().map(String::from)).collect()
}

/// The parent issue, an epic to the board, when it has a number.
fn parent_of(content: &Value) -> Option<IssueRef> {
    let p = &content["parent"];
    numbered(p).map(|key| IssueRef {
        key,
        title: p["title"].as_str().unwrap_or("").into(),
        kind: IssueKind::Epic,
        status: None,
    })
}

/// An issue, pull request or draft as the domain has it. `fields` are the
/// project item's values, absent for a search hit, whose status then
/// comes from open or closed.
fn issue_of(
    content: &Value,
    fields: &[Value],
    item: Option<&str>,
    status_field: &str,
    ranks: &HashMap<String, Priority>,
) -> Option<Known> {
    let kind = content["__typename"].as_str()?.to_string();
    let content_id = content["id"].as_str()?.to_string();
    let key = numbered(content).unwrap_or_else(|| draft_key(item.unwrap_or(&content_id)));
    let labels = labels_of(content);
    let read = ItemFields::read(fields, status_field, ranks);
    let created = datetime(&content["createdAt"])?;
    let author = content.get("author").or(content.get("creator"));
    let issue = Issue {
        id: IssueId::new(item.unwrap_or(&content_id)),
        key,
        title: content["title"].as_str().unwrap_or("").into(),
        kind: issue_type(&kind, &labels),
        status: read.status.unwrap_or_else(|| content_status(content, item)),
        priority: read.priority,
        assignee: content["assignees"]["nodes"].as_array().and_then(|a| a.first()).and_then(user),
        reporter: author.and_then(user),
        labels,
        parent: parent_of(content),
        subtasks: vec![],
        links: vec![],
        estimate: read.estimate,
        sprint: read.sprint,
        created,
        updated: datetime(&content["updatedAt"]).unwrap_or(created),
        due: None,
        description: Some(RichText::from_markdown(content["body"].as_str().unwrap_or(""))),
        fields: read.custom,
        url: content["url"].as_str().map(String::from),
        // GitHub keeps files as links in the Markdown body, not as a list.
        attachments: vec![],
    };
    Some(Known { item: item.map(String::from), content: content_id, kind, issue })
}

fn item_of(node: &Value, status_field: &str, ranks: &HashMap<String, Priority>) -> Option<Known> {
    let fields = node["fieldValues"]["nodes"].as_array().cloned().unwrap_or_default();
    issue_of(&node["content"], &fields, node["id"].as_str(), status_field, ranks)
}

impl GithubProvider {
    pub fn new(settings: GithubSettings) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("kolum/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        GithubProvider {
            http,
            settings,
            project: OnceCell::new(),
            known: Mutex::new(HashMap::new()),
            order: Mutex::new(vec![]),
        }
    }

    pub fn from_profile(table: &toml::Table) -> std::result::Result<Self, String> {
        let env = std::env::var("KOLUM_TOKEN").or_else(|_| std::env::var("GITHUB_TOKEN")).ok();
        Ok(Self::new(settings::parse(table, env)?))
    }

    /// The project itself, `owner/12`, as the profile names it: it holds
    /// the board but numbers no issue, each is numbered in its repository.
    /// Its owner would not do: a repository can be named as its owner is.
    fn board_project(&self, p: &ProjectInfo) -> Project {
        let key = format!("{}/{}", p.owner, self.settings.number);
        Project { key, name: p.owner.clone() }
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let response = self
            .http
            .post(&self.settings.api_url)
            .bearer_auth(&self.settings.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(transport)?;
        graphql_result(status, &body)
    }

    async fn project(&self) -> Result<&ProjectInfo> {
        self.project
            .get_or_try_init(|| async {
                let vars = json!({ "owner": self.settings.owner, "number": self.settings.number });
                let data = self.graphql(queries::PROJECT, vars).await?;
                let p = [&data["user"]["projectV2"], &data["organization"]["projectV2"]]
                    .into_iter()
                    .find(|p| p.is_object())
                    .ok_or_else(|| {
                        DomainError::NotFound(format!(
                            "project {}/{}",
                            self.settings.owner, self.settings.number
                        ))
                    })?;
                let fields = p["fields"]["nodes"].as_array().map(Vec::as_slice).unwrap_or_default();
                let status = fields.iter().find(|f| {
                    f["name"].as_str() == Some(&self.settings.status_field)
                        && f["options"].is_array()
                });
                let options = status
                    .and_then(|s| s["options"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|o| {
                        Some((o["id"].as_str()?.to_string(), o["name"].as_str()?.to_string()))
                    })
                    .collect();
                Ok(ProjectInfo {
                    id: p["id"].as_str().unwrap_or("").into(),
                    title: p["title"].as_str().unwrap_or("").into(),
                    owner: self.settings.owner.clone(),
                    status_field: status.and_then(|s| s["id"].as_str()).map(String::from),
                    options,
                    priorities: priority_ranks(fields),
                })
            })
            .await
    }

    fn remember(&self, known: &[Known]) {
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        for k in known {
            map.insert(k.issue.key.clone(), k.clone());
        }
    }

    fn known(&self, key: &IssueKey) -> Result<Known> {
        let map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key).cloned().ok_or_else(|| {
            DomainError::NotFound(format!("{key} is not on the board or in a search"))
        })
    }

    /// Its project item: a search hit off the board has none.
    fn item(&self, key: &IssueKey) -> Result<String> {
        self.known(key)?
            .item
            .ok_or_else(|| DomainError::NotFound(format!("{key} is not in the project")))
    }
}

#[async_trait]
impl BoardSource for GithubProvider {
    async fn boards(&self) -> Result<Vec<BoardSummary>> {
        let p = self.project().await?;
        let project = self.board_project(p);
        Ok(vec![BoardSummary {
            id: BoardId::new(&p.id),
            name: p.title.clone(),
            kind: BoardKind::Kanban,
            project: Some(project),
        }])
    }

    async fn board(&self, id: &BoardId) -> Result<Board> {
        let p = self.project().await?;
        if id.as_str() != p.id {
            return Err(DomainError::NotFound(format!("board {id}")));
        }
        let mut columns = vec![Column {
            name: "No Status".into(),
            statuses: vec![StatusId::new("")],
            wip_limit: None,
        }];
        columns.extend(p.options.iter().map(|(id, name)| Column {
            name: name.clone(),
            statuses: vec![StatusId::new(id)],
            wip_limit: None,
        }));
        Ok(Board {
            id: id.clone(),
            name: p.title.clone(),
            kind: BoardKind::Kanban,
            project: Some(self.board_project(p)),
            columns,
        })
    }

    /// GraphQL cannot filter items by date, so there is no `ChangedSince`:
    /// every sync reads every page, and `since` is never given.
    async fn board_issues(
        &self,
        id: &BoardId,
        _since: Option<OffsetDateTime>,
        page: Option<PageToken>,
    ) -> Result<Page<Issue>> {
        let first = page.is_none();
        let ranks = &self.project().await?.priorities;
        let data = self
            .graphql(&queries::items(), json!({ "id": id.as_str(), "after": page.map(|p| p.0) }))
            .await?;
        let items = &data["node"]["items"];
        let known: Vec<Known> = items["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| item_of(n, &self.settings.status_field, ranks))
            .collect();
        self.remember(&known);
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        if first {
            order.clear();
        }
        order.extend(known.iter().filter_map(|k| k.item.clone()));
        drop(order);
        let next = (items["pageInfo"]["hasNextPage"] == true)
            .then(|| items["pageInfo"]["endCursor"].as_str().map(|c| PageToken(c.into())))
            .flatten();
        Ok(Page { items: known.into_iter().map(|k| k.issue).collect(), next })
    }
}

#[async_trait]
impl IssueReader for GithubProvider {
    async fn issue(&self, key: &IssueKey) -> Result<Issue> {
        let known = self.known(key)?;
        let Some(item) = &known.item else { return Ok(known.issue) };
        let ranks = &self.project().await?.priorities;
        let data = self.graphql(&queries::item(), json!({ "id": item })).await?;
        let fresh = item_of(&data["node"], &self.settings.status_field, ranks)
            .ok_or_else(|| DomainError::NotFound(key.to_string()))?;
        self.remember(std::slice::from_ref(&fresh));
        Ok(fresh.issue)
    }

    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>> {
        let known = self.known(key)?;
        if known.kind == "DraftIssue" {
            return Ok(vec![]);
        }
        let data = self.graphql(queries::COMMENTS, json!({ "id": known.content })).await?;
        Ok(data["node"]["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(comment)
            .collect())
    }

    /// Not read yet: GitHub's timeline is its own large query.
    async fn activity(&self, _: &IssueKey) -> Result<Vec<ActivityEntry>> {
        Ok(vec![])
    }
}

fn comment(c: &Value) -> Option<Comment> {
    Some(Comment {
        id: c["id"].as_str()?.into(),
        author: user(&c["author"]).unwrap_or(User {
            id: UserId::new(""),
            display_name: "ghost".into(),
            avatar_url: None,
        }),
        body: RichText::from_markdown(c["body"].as_str().unwrap_or("")),
        created: datetime(&c["createdAt"])?,
    })
}

/// The mutation that edits the title and body of this kind of content.
fn update_mutation(kind: &str) -> &'static str {
    match kind {
        "PullRequest" => queries::UPDATE_PULL,
        "DraftIssue" => queries::UPDATE_DRAFT,
        _ => queries::UPDATE_ISSUE,
    }
}

/// The item a move puts `item` after to land before `other`: the one before
/// `other` in the project's order, skipping `item` itself; none puts it first.
fn item_before(order: &[String], other: &str, item: &str) -> Result<Option<String>> {
    let at = order
        .iter()
        .position(|i| i == other)
        .ok_or_else(|| DomainError::NotFound(other.to_string()))?;
    Ok(order[..at].iter().rev().find(|i| *i != item).cloned())
}

fn quote(v: &str) -> String {
    if v.contains(' ') { format!("\"{}\"", v.replace('"', "")) } else { v.to_string() }
}

/// Alternatives: GitHub's issue search takes OR and parentheses.
fn assignees_term(assignees: &[UserSel]) -> Option<String> {
    let who: Vec<String> = assignees
        .iter()
        .map(|sel| match sel {
            UserSel::Me => "assignee:@me".to_string(),
            UserSel::Named(n) => format!("assignee:{n}"),
            UserSel::Nobody => "no:assignee".to_string(),
        })
        .collect();
    match who.len() {
        0 => None,
        1 => who.into_iter().next(),
        _ => Some(format!("({})", who.join(" OR "))),
    }
}

/// A pull request or an issue by its kind; any other type is a label.
fn type_term(kind: &str) -> String {
    match kind.to_lowercase().as_str() {
        "pr" | "pull" | "pullrequest" => "is:pr".to_string(),
        "issue" => "is:issue".to_string(),
        other => format!("label:{}", quote(other)),
    }
}

/// The neutral query in GitHub's search syntax, inside the project.
pub fn search_query(query: &Query, owner: &str, number: u64) -> String {
    let filter = match query {
        Query::Native(q) => return q.clone(),
        Query::Filter(f) => f,
    };
    let mut parts = vec![format!("project:{owner}/{number}")];
    parts.extend(filter.text.clone());
    parts.extend(assignees_term(&filter.assignees));
    parts.extend(filter.labels.iter().map(|l| format!("label:{}", quote(l))));
    parts.extend(filter.types.iter().map(|t| type_term(t)));
    if filter.statuses.iter().any(|s| category(s) == StatusCategory::Done) {
        parts.push("is:closed".into());
    }
    parts.join(" ")
}

#[async_trait]
impl IssueSearch for GithubProvider {
    async fn search(&self, query: &Query, page: Option<PageToken>) -> Result<Page<Issue>> {
        let q = search_query(query, &self.settings.owner, self.settings.number);
        let data =
            self.graphql(queries::SEARCH, json!({ "q": q, "after": page.map(|p| p.0) })).await?;
        let search = &data["search"];
        let hits =
            search["nodes"].as_array().into_iter().flatten().filter_map(|n| {
                issue_of(n, &[], None, &self.settings.status_field, &HashMap::new())
            });
        let next = (search["pageInfo"]["hasNextPage"] == true)
            .then(|| search["pageInfo"]["endCursor"].as_str().map(|c| PageToken(c.into())))
            .flatten();
        // A hit already on the board keeps what the board knows of it
        // (its status); a new one is remembered, to be opened later.
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let items =
            hits.map(|hit| map.entry(hit.issue.key.clone()).or_insert(hit).issue.clone()).collect();
        Ok(Page { items, next })
    }
}

#[async_trait]
impl IssueWriter for GithubProvider {
    async fn transitions(&self, key: &IssueKey) -> Result<Vec<Transition>> {
        let current = self.known(key)?.issue.status.id;
        let p = self.project().await?;
        Ok(p.options
            .iter()
            .filter(|(id, _)| *id != current.as_str())
            .map(|(id, name)| Transition {
                id: TransitionId::new(id),
                name: name.clone(),
                to: Status { id: StatusId::new(id), name: name.clone(), category: category(name) },
                required_fields: vec![],
            })
            .collect())
    }

    async fn transition(&self, key: &IssueKey, transition: &TransitionId) -> Result<()> {
        // Off the board says so before any request: nothing to ask GitHub.
        let item = self.item(key)?;
        let p = self.project().await?;
        let field = p.status_field.clone().ok_or_else(|| {
            DomainError::NotFound(format!("a `{}` field", self.settings.status_field))
        })?;
        let vars =
            json!({ "project": p.id, "item": item, "field": field, "option": transition.as_str() });
        self.graphql(queries::SET_STATUS, vars).await.map(drop)
    }

    /// One assignee, as the board shows one: the others are removed.
    async fn assign(&self, key: &IssueKey, who: Option<&UserId>) -> Result<()> {
        let known = self.known(key)?;
        if known.kind == "DraftIssue" {
            return Err(DomainError::Unsupported(Capability::Write));
        }
        if let Some(current) = &known.issue.assignee {
            self.graphql(
                queries::REMOVE_ASSIGNEES,
                json!({ "id": known.content, "users": [current.id.as_str()] }),
            )
            .await?;
        }
        if let Some(who) = who {
            self.graphql(
                queries::ADD_ASSIGNEES,
                json!({ "id": known.content, "users": [who.as_str()] }),
            )
            .await?;
        }
        Ok(())
    }

    async fn comment(&self, key: &IssueKey, body: RichText) -> Result<Comment> {
        let known = self.known(key)?;
        let data = self
            .graphql(
                queries::ADD_COMMENT,
                json!({ "id": known.content, "body": body.to_markdown() }),
            )
            .await?;
        comment(&data["addComment"]["commentEdge"]["node"])
            .ok_or_else(|| transport("GitHub sent back an unreadable comment"))
    }

    /// The title and the body. Labels and priority live elsewhere on
    /// GitHub (label ids, a project field) and are refused for now.
    async fn update(&self, key: &IssueKey, update: IssueUpdate) -> Result<()> {
        if update.labels.is_some() || update.priority.is_some() || !update.fields.is_empty() {
            return Err(DomainError::Unsupported(Capability::CustomFields));
        }
        if update.title.is_none() && update.description.is_none() {
            return Ok(());
        }
        let known = self.known(key)?;
        let mutation = update_mutation(&known.kind);
        let vars = json!({ "id": known.content, "title": update.title, "body": update.description.map(|d| d.to_markdown()) });
        self.graphql(mutation, vars).await.map(drop)
    }

    /// A draft issue in the project: it needs no repository.
    async fn create(&self, new: NewIssue) -> Result<Issue> {
        let p = self.project().await?;
        let vars =
            json!({ "project": p.id, "title": new.title, "body": new.description.to_markdown() });
        let data = self.graphql(queries::ADD_DRAFT, vars).await?;
        let item = data["addProjectV2DraftIssue"]["projectItem"]["id"]
            .as_str()
            .ok_or_else(|| transport("GitHub created no item"))?;
        let data = self.graphql(&queries::item(), json!({ "id": item })).await?;
        let known = item_of(&data["node"], &self.settings.status_field, &p.priorities)
            .ok_or_else(|| transport("an unreadable new item"))?;
        self.remember(std::slice::from_ref(&known));
        Ok(known.issue)
    }

    async fn rank(&self, key: &IssueKey, rank: &Rank) -> Result<()> {
        let p = self.project().await?;
        let item = self.item(key)?;
        let (Rank::Before(other) | Rank::After(other)) = rank;
        let other = self.item(other)?;
        let after = match rank {
            Rank::After(_) => Some(other),
            Rank::Before(_) => {
                let order = self.order.lock().unwrap_or_else(|e| e.into_inner());
                item_before(&order, &other, &item)?
            }
        };
        let vars = json!({ "project": p.id, "item": item, "after": after });
        self.graphql(queries::MOVE_ITEM, vars).await.map(drop)
    }

    async fn clone_issue(&self, key: &IssueKey) -> Result<Issue> {
        let source = self.known(key)?.issue;
        let new = NewIssue {
            project: self.settings.owner.clone(),
            kind: source.kind,
            title: format!("CLONE - {}", source.title),
            description: source.description.unwrap_or_default(),
            parent: None,
        };
        self.create(new).await
    }
}

#[async_trait]
impl UserDirectory for GithubProvider {
    async fn me(&self) -> Result<User> {
        let data = self.graphql(queries::VIEWER, json!({})).await?;
        user(&data["viewer"]).ok_or_else(|| transport("GitHub did not say who you are"))
    }

    async fn find_users(&self, query: &str) -> Result<Vec<User>> {
        let data = self.graphql(queries::USERS, json!({ "q": query })).await?;
        Ok(data["search"]["nodes"].as_array().into_iter().flatten().filter_map(user).collect())
    }

    /// Avatars are public: no token goes with the request.
    async fn image(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.http.get(url).send().await.map_err(transport)?;
        if !response.status().is_success() {
            return Err(DomainError::NotFound(format!("{url}: HTTP {}", response.status())));
        }
        Ok(response.bytes().await.map_err(transport)?.to_vec())
    }
}

impl Provider for GithubProvider {
    fn name(&self) -> &str {
        "github"
    }

    fn capabilities(&self) -> Capabilities {
        use Capability::*;
        Capabilities::new([
            Sprints,
            Epics,
            Estimates,
            NativeQuery,
            CustomFields,
            Comments,
            Write,
            Images,
            Rank,
        ])
    }
}

#[cfg(test)]
mod tests;
