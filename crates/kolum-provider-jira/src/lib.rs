//! The Jira adapter: Cloud's REST v3 and Agile 1.0 APIs, with the paths and
//! authentication switching to v2 and a Bearer token on Data Center.
//!
//! Reads and writes: transitions, assigning, comments, edits, creating
//! and cloning. Data Center's rich text is wiki markup, read as plain text
//! and written as Markdown until it has a reader and a writer of its own.

mod adf;
mod client;
mod dto;
pub mod jql;
mod map;
mod settings;

pub use settings::{JiraFlavor, JiraSettings};

use async_trait::async_trait;
use client::{Client, Method};
use dto::*;
use kolum_domain::*;
use map::Ctx;
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::OnceCell;

const AGILE: &str = "/rest/agile/1.0";
const PAGE: u64 = 100;
const SEARCH_PAGE: u64 = 50;
/// What a card, the picker and the detail need. Custom fields are added
/// once known; `*all` would also bring every plugin's fields on every card.
const FIELDS: &str = "summary,issuetype,status,priority,assignee,reporter,labels,parent,subtasks,\
                      issuelinks,created,updated,duedate,description,timeoriginalestimate,attachment";

pub struct JiraProvider {
    client: Client,
    settings: JiraSettings,
    ctx: OnceCell<Ctx>,
}

impl JiraProvider {
    pub fn new(settings: JiraSettings) -> Self {
        JiraProvider {
            client: Client::new(&settings.base_url, &settings.auth),
            settings,
            ctx: OnceCell::new(),
        }
    }

    /// From a profile's settings, the token resolved from the file, the
    /// command or `$KOLUM_TOKEN`.
    pub fn from_profile(table: &toml::Table) -> std::result::Result<Self, String> {
        let settings = settings::parse(table, std::env::var("KOLUM_TOKEN").ok())?;
        Ok(Self::new(settings))
    }

    /// The board the profile names, if it names one.
    pub fn board_id(&self) -> Option<BoardId> {
        self.settings.board.map(|b| BoardId::new(b.to_string()))
    }

    /// Rich text as this Jira takes it: ADF on Cloud. Data Center wants
    /// wiki markup, which kolum does not write yet; Markdown reads close.
    fn rich(&self, rt: &RichText) -> serde_json::Value {
        match self.settings.flavor {
            JiraFlavor::Cloud => adf::from_rich_text(rt),
            JiraFlavor::DataCenter => json!(rt.to_markdown()),
        }
    }

    fn api(&self) -> &'static str {
        self.settings.flavor.api()
    }

    /// The custom field ids and names, read once per session.
    async fn ctx(&self) -> Result<&Ctx> {
        self.ctx
            .get_or_try_init(|| async {
                let fields: Vec<FieldDto> =
                    self.client.get(&format!("{}/field", self.api()), &[]).await?;
                Ok(Ctx {
                    base_url: self.settings.base_url.clone(),
                    fields: map::detect_fields(&fields, &self.settings.fields),
                    names: map::custom_names(&fields),
                    schemas: map::schemas(&fields),
                })
            })
            .await
    }

    async fn fields_param(&self) -> Result<String> {
        let ids = &self.ctx().await?.fields;
        let extra = [&ids.estimate, &ids.sprint, &ids.epic_link].into_iter().flatten();
        Ok(std::iter::once(FIELDS.to_string()).chain(extra.cloned()).collect::<Vec<_>>().join(","))
    }

    async fn issues(&self, dtos: &[IssueDto]) -> Result<Vec<Issue>> {
        let ctx = self.ctx().await?;
        dtos.iter().map(|d| map::issue(d, ctx)).collect()
    }

    /// Every `@name` resolved to account ids, for JQL.
    async fn assignee_ids(&self, query: &Query) -> Result<Vec<String>> {
        let Query::Filter(filter) = query else { return Ok(vec![]) };
        let mut ids = vec![];
        for sel in &filter.assignees {
            if let UserSel::Named(name) = sel {
                ids.extend(self.find_users(name).await?.into_iter().map(|u| u.id.0));
            }
        }
        Ok(ids)
    }
}

/// The reasons in a 207 answer to a rank, if any issue was refused.
pub fn rank_errors(answer: &serde_json::Value) -> Option<String> {
    let refused: Vec<String> = answer["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["status"].as_u64().is_some_and(|s| s >= 300))
        .flat_map(|e| e["errors"].as_array().cloned().unwrap_or_default())
        .filter_map(|m| m.as_str().map(String::from))
        .collect();
    (!refused.is_empty()).then(|| refused.join("; "))
}

fn offset_page(page: &OffsetPage) -> Option<PageToken> {
    let next = page.start_at + page.issues.len() as u64;
    (!page.issues.is_empty() && next < page.total).then(|| PageToken(next.to_string()))
}

fn start_at(token: &Option<PageToken>) -> String {
    token.as_ref().map_or("0".into(), |t| t.0.clone())
}

#[async_trait]
impl BoardSource for JiraProvider {
    async fn boards(&self) -> Result<Vec<BoardSummary>> {
        let mut out = vec![];
        loop {
            let query = [("startAt", out.len().to_string()), ("maxResults", "50".to_string())];
            let page: BoardPage = self.client.get(&format!("{AGILE}/board"), &query).await?;
            let last = page.is_last || page.values.is_empty();
            out.extend(page.values.iter().map(map::summary));
            if last {
                return Ok(out);
            }
        }
    }

    async fn board(&self, id: &BoardId) -> Result<Board> {
        let (summary_path, config_path) =
            (format!("{AGILE}/board/{id}"), format!("{AGILE}/board/{id}/configuration"));
        let (summary, config) = tokio::try_join!(
            self.client.get::<BoardDto>(&summary_path, &[]),
            self.client.get::<BoardConfigDto>(&config_path, &[]),
        )?;
        Ok(map::board(&summary, &config))
    }

    async fn board_issues(
        &self,
        id: &BoardId,
        since: Option<OffsetDateTime>,
        token: Option<PageToken>,
    ) -> Result<Page<Issue>> {
        let mut query = vec![
            ("startAt", start_at(&token)),
            ("maxResults", PAGE.to_string()),
            ("fields", self.fields_param().await?),
        ];
        if let Some(since) = since {
            query.push(("jql", jql::updated_since(since)));
        }
        let page: OffsetPage =
            self.client.get(&format!("{AGILE}/board/{id}/issue"), &query).await?;
        Ok(Page { next: offset_page(&page), items: self.issues(&page.issues).await? })
    }
}

#[derive(serde::Deserialize)]
struct WithChangelog {
    changelog: Option<Histories>,
}

#[derive(serde::Deserialize)]
struct Histories {
    histories: Vec<HistoryDto>,
}

#[async_trait]
impl IssueReader for JiraProvider {
    async fn issue(&self, key: &IssueKey) -> Result<Issue> {
        let ctx = self.ctx().await?;
        let dto: IssueDto = self
            .client
            .get(&format!("{}/issue/{key}", self.api()), &[("fields", "*all".into())])
            .await?;
        map::issue(&dto, ctx)
    }

    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>> {
        let query = [("orderBy", "created".to_string()), ("maxResults", PAGE.to_string())];
        let page: CommentPage =
            self.client.get(&format!("{}/issue/{key}/comment", self.api()), &query).await?;
        Ok(page.comments.iter().filter_map(map::comment).collect())
    }

    async fn activity(&self, key: &IssueKey) -> Result<Vec<ActivityEntry>> {
        let query = [("fields", "summary".to_string()), ("expand", "changelog".to_string())];
        let dto: WithChangelog =
            self.client.get(&format!("{}/issue/{key}", self.api()), &query).await?;
        let histories = dto.changelog.map(|c| c.histories).unwrap_or_default();
        Ok(histories.iter().filter_map(map::activity).collect())
    }
}

#[async_trait]
impl IssueSearch for JiraProvider {
    /// Jira's `text ~` never matches a key: a query that is only a key also
    /// looks that issue up, and puts it first.
    async fn search(&self, query: &Query, token: Option<PageToken>) -> Result<Page<Issue>> {
        let key = match (query, &token) {
            (Query::Filter(f), None) if !f.has_fields() => {
                f.text.as_deref().and_then(jql::issue_key)
            }
            _ => None,
        };
        let mut page = self.search_jql(query, token).await?;
        let Some(key) = key else { return Ok(page) };
        match self.issue(&key).await {
            Ok(issue) => {
                page.items.retain(|i| i.key != issue.key);
                page.items.insert(0, issue);
            }
            Err(DomainError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        Ok(page)
    }
}

impl JiraProvider {
    async fn search_jql(&self, query: &Query, token: Option<PageToken>) -> Result<Page<Issue>> {
        let jql = match query {
            Query::Native(jql) => jql.clone(),
            Query::Filter(filter) => {
                let ids = self.assignee_ids(query).await?;
                // Only names, and none of them anyone: nothing can match.
                let only_names = !filter.assignees.is_empty()
                    && filter.assignees.iter().all(|s| matches!(s, UserSel::Named(_)));
                if only_names && ids.is_empty() {
                    return Ok(Page { items: vec![], next: None });
                }
                jql::filter(filter, &ids)
            }
        };
        let mut params = vec![
            ("jql", jql),
            ("maxResults", SEARCH_PAGE.to_string()),
            ("fields", self.fields_param().await?),
        ];
        match self.settings.flavor {
            JiraFlavor::Cloud => {
                if let Some(t) = token {
                    params.push(("nextPageToken", t.0));
                }
                let page: TokenPage =
                    self.client.get(&format!("{}/search/jql", self.api()), &params).await?;
                Ok(Page {
                    next: page.next_page_token.map(PageToken),
                    items: self.issues(&page.issues).await?,
                })
            }
            JiraFlavor::DataCenter => {
                params.push(("startAt", start_at(&token)));
                let page: OffsetPage =
                    self.client.get(&format!("{}/search", self.api()), &params).await?;
                Ok(Page { next: offset_page(&page), items: self.issues(&page.issues).await? })
            }
        }
    }
}

#[async_trait]
impl IssueWriter for JiraProvider {
    async fn transitions(&self, key: &IssueKey) -> Result<Vec<Transition>> {
        let query = [("expand", "transitions.fields".to_string())];
        let page: TransitionPage =
            self.client.get(&format!("{}/issue/{key}/transitions", self.api()), &query).await?;
        Ok(page.transitions.iter().map(map::transition).collect())
    }

    async fn transition(&self, key: &IssueKey, transition: &TransitionId) -> Result<()> {
        let body = json!({ "transition": { "id": transition.as_str() } });
        self.client
            .send(Method::POST, &format!("{}/issue/{key}/transitions", self.api()), &body)
            .await
            .map(drop)
    }

    /// Cloud names a user by account id, Data Center by user name; `null`
    /// unassigns either way.
    async fn assign(&self, key: &IssueKey, user: Option<&UserId>) -> Result<()> {
        let field = match self.settings.flavor {
            JiraFlavor::Cloud => "accountId",
            JiraFlavor::DataCenter => "name",
        };
        let body = json!({ field: user.map(|u| u.as_str()) });
        self.client
            .send(Method::PUT, &format!("{}/issue/{key}/assignee", self.api()), &body)
            .await
            .map(drop)
    }

    async fn comment(&self, key: &IssueKey, body: RichText) -> Result<Comment> {
        let body = json!({ "body": self.rich(&body) });
        let dto = self
            .client
            .send(Method::POST, &format!("{}/issue/{key}/comment", self.api()), &body)
            .await?;
        let dto: CommentDto =
            serde_json::from_value(dto).map_err(|e| DomainError::Transport(e.to_string()))?;
        map::comment(&dto)
            .ok_or_else(|| DomainError::Transport("Jira sent back an unreadable comment".into()))
    }

    async fn update(&self, key: &IssueKey, update: IssueUpdate) -> Result<()> {
        let mut fields = serde_json::Map::new();
        if let Some(title) = update.title {
            fields.insert("summary".into(), json!(title));
        }
        if let Some(description) = &update.description {
            fields.insert("description".into(), self.rich(description));
        }
        if let Some(labels) = update.labels {
            fields.insert("labels".into(), json!(labels));
        }
        if let Some(priority) = update.priority {
            fields.insert("priority".into(), json!({ "name": priority.name() }));
        }
        let ctx = self.ctx().await?;
        let user_key = match self.settings.flavor {
            JiraFlavor::Cloud => "accountId",
            JiraFlavor::DataCenter => "name",
        };
        for (id, value) in &update.fields {
            let schema = ctx
                .schemas
                .get(id.as_str())
                .ok_or_else(|| DomainError::Transport(format!("Jira lists no field {id}")))?;
            let rich = |t: &RichText| self.rich(t);
            fields.insert(id.to_string(), map::field_json(schema, value.as_ref(), user_key, &rich));
        }
        if fields.is_empty() {
            return Ok(());
        }
        let body = json!({ "fields": fields });
        self.client.send(Method::PUT, &format!("{}/issue/{key}", self.api()), &body).await.map(drop)
    }

    async fn create(&self, new: NewIssue) -> Result<Issue> {
        let mut fields = json!({
            "project": { "key": new.project },
            "issuetype": { "name": new.kind.name },
            "summary": new.title,
        });
        if !new.description.is_empty() {
            fields["description"] = self.rich(&new.description);
        }
        if let Some(parent) = new.parent {
            fields["parent"] = json!({ "key": parent.as_str() });
        }
        let created = self
            .client
            .send(Method::POST, &format!("{}/issue", self.api()), &json!({ "fields": fields }))
            .await?;
        let key = created["key"]
            .as_str()
            .ok_or_else(|| DomainError::Transport("Jira created an issue with no key".into()))?;
        self.issue(&IssueKey::new(key)).await
    }

    /// Jira's own rank, the board's order. It answers 207 when it refuses
    /// an issue, with the reason inside: that is a failure, not a success.
    async fn rank(&self, key: &IssueKey, rank: &Rank) -> Result<()> {
        let body = match rank {
            Rank::Before(other) => {
                json!({ "issues": [key.as_str()], "rankBeforeIssue": other.as_str() })
            }
            Rank::After(other) => {
                json!({ "issues": [key.as_str()], "rankAfterIssue": other.as_str() })
            }
        };
        let answer = self.client.send(Method::PUT, &format!("{AGILE}/issue/rank"), &body).await?;
        match rank_errors(&answer) {
            Some(why) => Err(DomainError::Conflict(why)),
            None => Ok(()),
        }
    }

    async fn clone_issue(&self, key: &IssueKey) -> Result<Issue> {
        let source = self.issue(key).await?;
        let project = key.as_str().split('-').next().unwrap_or_default().to_string();
        let new = NewIssue {
            project,
            kind: source.kind,
            title: format!("CLONE - {}", source.title),
            description: source.description,
            parent: source.parent.map(|p| p.key),
        };
        self.create(new).await
    }
}

#[async_trait]
impl UserDirectory for JiraProvider {
    async fn me(&self) -> Result<User> {
        let dto: UserDto = self.client.get(&format!("{}/myself", self.api()), &[]).await?;
        Ok(map::user(&dto))
    }

    async fn find_users(&self, query: &str) -> Result<Vec<User>> {
        let param = match self.settings.flavor {
            JiraFlavor::Cloud => "query",
            JiraFlavor::DataCenter => "username",
        };
        let users: Vec<UserDto> = self
            .client
            .get(&format!("{}/user/search", self.api()), &[(param, query.to_string())])
            .await?;
        Ok(users.iter().map(map::user).collect())
    }

    async fn image(&self, url: &str) -> Result<Vec<u8>> {
        self.client.bytes(url).await
    }
}

impl Provider for JiraProvider {
    fn name(&self) -> &str {
        "jira"
    }

    fn capabilities(&self) -> Capabilities {
        use Capability::*;
        Capabilities::new([
            Sprints,
            Epics,
            Estimates,
            WipLimits,
            NativeQuery,
            CustomFields,
            Comments,
            Activity,
            Write,
            Images,
            Rank,
        ])
    }
}

#[cfg(test)]
mod tests;
