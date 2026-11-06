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
use std::collections::HashMap;
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
    let message = || parsed["message"].as_str().unwrap_or("").to_string();
    match status {
        401 => return Err(DomainError::Unauthorized(message())),
        403 if message().to_lowercase().contains("rate limit") => {
            return Err(DomainError::RateLimited { retry_after_secs: None });
        }
        200..=299 => {}
        _ => return Err(DomainError::Transport(format!("HTTP {status}: {}", message()))),
    }
    let errors = parsed["errors"].as_array().cloned().unwrap_or_default();
    let data = parsed["data"].clone();
    let has_data = data.as_object().is_some_and(|d| d.values().any(|v| !v.is_null()));
    match errors.first() {
        Some(e) if !has_data => {
            let text = e["message"].as_str().unwrap_or("GraphQL error").to_string();
            Err(match e["type"].as_str() {
                Some("NOT_FOUND") => DomainError::NotFound(text),
                Some("FORBIDDEN") | Some("INSUFFICIENT_SCOPES") => DomainError::Unauthorized(text),
                Some("RATE_LIMITED") => DomainError::RateLimited { retry_after_secs: None },
                _ => DomainError::Transport(text),
            })
        }
        _ => Ok(data),
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

fn priority(name: &str) -> Option<Priority> {
    Some(match name.to_lowercase().as_str() {
        "p0" | "urgent" | "critical" | "highest" => Priority::Highest,
        "p1" | "high" => Priority::High,
        "p2" | "medium" | "normal" => Priority::Medium,
        "p3" | "low" => Priority::Low,
        "p4" | "lowest" => Priority::Lowest,
        _ => return None,
    })
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

/// An issue, pull request or draft as the domain has it. `fields` are the
/// project item's values, absent for a search hit, whose status then
/// comes from open or closed.
fn issue_of(
    content: &Value,
    fields: &[Value],
    item: Option<&str>,
    status_field: &str,
) -> Option<Known> {
    let kind = content["__typename"].as_str()?.to_string();
    let content_id = content["id"].as_str()?.to_string();
    let key = numbered(content).unwrap_or_else(|| draft_key(item.unwrap_or(&content_id)));
    let labels: Vec<String> = content["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect();
    let issue_type = match &*kind {
        "PullRequest" => IssueType { name: "Pull request".into(), kind: IssueKind::Other },
        "DraftIssue" => IssueType { name: "Draft".into(), kind: IssueKind::Task },
        _ if labels.iter().any(|l| l.to_lowercase().contains("bug")) => {
            IssueType { name: "Bug".into(), kind: IssueKind::Bug }
        }
        _ => IssueType { name: "Issue".into(), kind: IssueKind::Task },
    };
    let field_name = |v: &Value| v["field"]["name"].as_str().unwrap_or("").to_string();
    let mut status = match content["state"].as_str() {
        Some("CLOSED" | "MERGED") if item.is_none() => Status {
            id: StatusId::new("closed"),
            name: "Closed".into(),
            category: StatusCategory::Done,
        },
        Some(_) if item.is_none() => Status {
            id: StatusId::new("open"),
            name: "Open".into(),
            category: StatusCategory::Todo,
        },
        _ => no_status(),
    };
    let (mut prio, mut estimate, mut sprint, mut custom) = (None, None, None, vec![]);
    for v in fields {
        let name = field_name(v);
        if let (Some(option), Some(value)) = (v["optionId"].as_str(), v["name"].as_str()) {
            if name == status_field {
                status = Status {
                    id: StatusId::new(option),
                    name: value.into(),
                    category: category(value),
                };
            } else if name.eq_ignore_ascii_case("priority") {
                prio = priority(value);
            } else {
                custom.push(CustomField {
                    id: FieldId::new(&name),
                    name: name.clone(),
                    value: FieldValue::Text(value.into()),
                });
            }
        } else if let Some(n) = v["number"].as_f64() {
            if ESTIMATE_FIELDS.contains(&name.to_lowercase().as_str()) {
                estimate = Some(Estimate::Points(n));
            } else {
                custom.push(CustomField {
                    id: FieldId::new(&name),
                    name: name.clone(),
                    value: FieldValue::Number(n),
                });
            }
        } else if let (Some(id), Some(title)) = (v["iterationId"].as_str(), v["title"].as_str()) {
            sprint = Some(Sprint { id: id.into(), name: title.into() });
        }
    }
    let created = datetime(&content["createdAt"])?;
    let p = &content["parent"];
    let parent = numbered(p).map(|key| IssueRef {
        key,
        title: p["title"].as_str().unwrap_or("").into(),
        kind: IssueKind::Epic,
        status: None,
    });
    let author = content.get("author").or(content.get("creator"));
    let issue = Issue {
        id: IssueId::new(item.unwrap_or(&content_id)),
        key,
        title: content["title"].as_str().unwrap_or("").into(),
        kind: issue_type,
        status,
        priority: prio,
        assignee: content["assignees"]["nodes"].as_array().and_then(|a| a.first()).and_then(user),
        reporter: author.and_then(user),
        labels,
        parent,
        subtasks: vec![],
        links: vec![],
        estimate,
        sprint,
        created,
        updated: datetime(&content["updatedAt"]).unwrap_or(created),
        due: None,
        description: RichText::from_markdown(content["body"].as_str().unwrap_or("")),
        fields: custom,
        url: content["url"].as_str().map(String::from),
        // GitHub keeps files as links in the Markdown body, not as a list.
        attachments: vec![],
    };
    Some(Known { item: item.map(String::from), content: content_id, kind, issue })
}

fn item_of(node: &Value, status_field: &str) -> Option<Known> {
    let fields = node["fieldValues"]["nodes"].as_array().cloned().unwrap_or_default();
    issue_of(&node["content"], &fields, node["id"].as_str(), status_field)
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
                let status = p["fields"]["nodes"].as_array().into_iter().flatten().find(|f| {
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
        let data = self
            .graphql(&queries::items(), json!({ "id": id.as_str(), "after": page.map(|p| p.0) }))
            .await?;
        let items = &data["node"]["items"];
        let known: Vec<Known> = items["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| item_of(n, &self.settings.status_field))
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
        let data = self.graphql(&queries::item(), json!({ "id": item })).await?;
        let fresh = item_of(&data["node"], &self.settings.status_field)
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

/// The neutral query in GitHub's search syntax, inside the project.
pub fn search_query(query: &Query, owner: &str, number: u64) -> String {
    let filter = match query {
        Query::Native(q) => return q.clone(),
        Query::Filter(f) => f,
    };
    let quote = |v: &str| {
        if v.contains(' ') { format!("\"{}\"", v.replace('"', "")) } else { v.to_string() }
    };
    let mut parts = vec![format!("project:{owner}/{number}")];
    parts.extend(filter.text.clone());
    let who: Vec<String> = filter
        .assignees
        .iter()
        .map(|sel| match sel {
            UserSel::Me => "assignee:@me".to_string(),
            UserSel::Named(n) => format!("assignee:{n}"),
            UserSel::Nobody => "no:assignee".to_string(),
        })
        .collect();
    match who.len() {
        0 => {}
        1 => parts.extend(who),
        // Alternatives: GitHub's issue search takes OR and parentheses.
        _ => parts.push(format!("({})", who.join(" OR "))),
    }
    parts.extend(filter.labels.iter().map(|l| format!("label:{}", quote(l))));
    parts.extend(filter.types.iter().map(|t| match t.to_lowercase().as_str() {
        "pr" | "pull" | "pullrequest" => "is:pr".to_string(),
        "issue" => "is:issue".to_string(),
        other => format!("label:{}", quote(other)),
    }));
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
        let hits = search["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| issue_of(n, &[], None, &self.settings.status_field));
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
        let known = self.known(key)?;
        let p = self.project().await?;
        let item = known
            .item
            .ok_or_else(|| DomainError::NotFound(format!("{key} is not in the project")))?;
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
        let mutation = match known.kind.as_str() {
            "PullRequest" => queries::UPDATE_PULL,
            "DraftIssue" => queries::UPDATE_DRAFT,
            _ => queries::UPDATE_ISSUE,
        };
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
        let known = item_of(&data["node"], &self.settings.status_field)
            .ok_or_else(|| transport("an unreadable new item"))?;
        self.remember(std::slice::from_ref(&known));
        Ok(known.issue)
    }

    async fn rank(&self, key: &IssueKey, rank: &Rank) -> Result<()> {
        let p = self.project().await?;
        let item = self
            .known(key)?
            .item
            .ok_or_else(|| DomainError::NotFound(format!("{key} is not in the project")))?;
        let (Rank::Before(other) | Rank::After(other)) = rank;
        let other = self
            .known(other)?
            .item
            .ok_or_else(|| DomainError::NotFound(format!("{other} is not in the project")))?;
        let after = match rank {
            Rank::After(_) => Some(other),
            Rank::Before(_) => {
                let order = self.order.lock().unwrap_or_else(|e| e.into_inner());
                let at = order
                    .iter()
                    .position(|i| *i == other)
                    .ok_or_else(|| DomainError::NotFound(other.clone()))?;
                // The one before it, skipping the item being moved; none
                // puts it first.
                order[..at].iter().rev().find(|i| **i != item).cloned()
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
            description: source.description,
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
