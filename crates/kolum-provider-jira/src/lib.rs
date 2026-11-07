//! The Jira adapter: Cloud's REST v3 and Agile 1.0 APIs, with the paths and
//! authentication switching to v2 and a Bearer token on Data Center.
//!
//! Reads and writes: transitions, assigning, comments, edits, creating
//! and cloning. Data Center's rich text is wiki markup, read by `wiki` and
//! written as Markdown until it has a writer of its own.

mod adf;
mod client;
mod dto;
pub mod jql;
mod map;
mod settings;
mod wiki;

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
    /// The site's id for Atlassian's GraphQL gateway.
    cloud: OnceCell<String>,
    /// What each board shows of its filter, by board id, as last read.
    scopes: std::sync::Mutex<std::collections::HashMap<String, jql::Scope>>,
}

/// The web issue view's field layout for one issue, and the fields its
/// user pinned in the project: Atlassian's GraphQL, the only place they are.
const LAYOUT_QUERY: &str = "query KolumFieldLayout($cloudId: ID!, $key: String!, $project: String!) { jira { issueByKey(key: $key, cloudId: $cloudId) { fieldsByContainerTypes(containerTypes: [CONTENT, PRIMARY, SECONDARY]) @optIn(to: \"JiraIssueFieldsByContainerTypes\") { ... on JiraIssueFieldsByContainerTypes { containerFields { containerType fields { edges { node { fieldId name type } } } } } ... on QueryError { message } } } userPreferences(cloudId: $cloudId) { issueViewPinnedFields(projectKey: $project) } } }";

impl JiraProvider {
    pub fn new(settings: JiraSettings) -> Self {
        JiraProvider {
            client: Client::new(&settings.base_url, &settings.auth),
            settings,
            ctx: OnceCell::new(),
            cloud: OnceCell::new(),
            scopes: Default::default(),
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

    /// How this Jira names a user: Cloud by account id, Data Center by
    /// user name.
    fn user_key(&self) -> &'static str {
        match self.settings.flavor {
            JiraFlavor::Cloud => "accountId",
            JiraFlavor::DataCenter => "name",
        }
    }

    /// The members of an edit, each in the field Jira writes it by.
    fn member_fields(&self, update: &IssueUpdate) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        if let Some(title) = &update.title {
            fields.insert("summary".into(), json!(title));
        }
        if let Some(description) = &update.description {
            fields.insert("description".into(), self.rich(description));
        }
        if let Some(labels) = &update.labels {
            fields.insert("labels".into(), json!(labels));
        }
        if let Some(priority) = &update.priority {
            fields.insert("priority".into(), json!({ "name": priority.name }));
        }
        fields
    }

    fn api(&self) -> &'static str {
        self.settings.flavor.api()
    }

    /// The custom field ids and names, and the priorities' order, read
    /// once per session.
    async fn ctx(&self) -> Result<&Ctx> {
        self.ctx
            .get_or_try_init(|| async {
                let paths = (format!("{}/field", self.api()), format!("{}/priority", self.api()));
                let (fields, priorities) = tokio::join!(
                    self.client.get::<Vec<FieldDto>>(&paths.0, &[]),
                    self.client.get::<Vec<NamedDto>>(&paths.1, &[]),
                );
                let fields = fields?;
                Ok(Ctx {
                    base_url: self.settings.base_url.clone(),
                    fields: map::detect_fields(&fields, &self.settings.fields),
                    names: map::custom_names(&fields),
                    schemas: map::schemas(&fields),
                    priorities: map::priorities(
                        priorities.inspect_err(|e| {
                            tracing::info!("priorities: {e}; ranked by their names")
                        }),
                    ),
                })
            })
            .await
    }

    /// The fields read into the issue's members, without failing: what the
    /// profile forces when Jira's list of fields cannot be read, so moving a
    /// card or `E` still work.
    async fn known(&self) -> map::FieldIds {
        match self.ctx().await {
            Ok(ctx) => ctx.fields.clone(),
            Err(_) => map::detect_fields(&[], &self.settings.fields),
        }
    }

    /// What the board shows of its filter: as last read with the board, or
    /// read now when the board itself was not.
    async fn scope(&self, id: &BoardId) -> Result<jql::Scope> {
        let known = self.scopes.lock().unwrap().get(id.as_str()).cloned();
        if let Some(scope) = known {
            return Ok(scope);
        }
        let config_path = format!("{AGILE}/board/{id}/configuration");
        let (config, done_cutoff) = tokio::join!(
            self.client.get::<BoardConfigDto>(&config_path, &[]),
            self.done_cutoff(id),
        );
        Ok(self.remember_scope(id, &config?, done_cutoff))
    }

    fn remember_scope(
        &self,
        id: &BoardId,
        config: &BoardConfigDto,
        done_cutoff: Option<String>,
    ) -> jql::Scope {
        let last = config.column_config.columns.last();
        let scope = jql::Scope {
            sub_query: config.sub_query.as_ref().map(|q| q.query.clone()),
            done_cutoff,
            done_statuses: last
                .map_or(vec![], |c| c.statuses.iter().map(|s| s.id.clone()).collect()),
        };
        self.scopes.lock().unwrap().insert(id.to_string(), scope.clone());
        scope
    }

    /// shortcut: "hide completed issues older than" is only in the private
    /// greenhopper API; read at best, the sub-filter alone when it does not
    /// answer. Read it from the public API if Atlassian ever puts it there.
    async fn done_cutoff(&self, id: &BoardId) -> Option<String> {
        let query = [("rapidViewId", id.to_string())];
        let path = "/rest/greenhopper/1.0/rapidviewconfig/editmodel.json";
        let model: EditModelDto = (self.client.get(path, &query).await)
            .inspect_err(|e| tracing::info!("{path}: {e}; completed issues kept"))
            .ok()?;
        model.old_done_issues_cutoff
    }

    /// A board page's fields: on Cloud without the description, which the
    /// Agile API sends as wiki markup; the issue's own read and the bulk read
    /// bring it whole.
    async fn board_fields(&self) -> Result<String> {
        let all = self.fields_param().await?;
        Ok(match self.settings.flavor {
            JiraFlavor::Cloud => {
                all.split(',').filter(|f| *f != "description").collect::<Vec<_>>().join(",")
            }
            JiraFlavor::DataCenter => all,
        })
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
        let (summary, config, done_cutoff) = tokio::join!(
            self.client.get::<BoardDto>(&summary_path, &[]),
            self.client.get::<BoardConfigDto>(&config_path, &[]),
            self.done_cutoff(id),
        );
        let (summary, config) = (summary?, config?);
        self.remember_scope(id, &config, done_cutoff);
        Ok(map::board(&summary, &config))
    }

    /// shortcut: one page of 50, past which a board has more sprints planned
    /// than anyone picks from; page like `boards` if that changes.
    async fn sprints(&self, board: &BoardId) -> Result<Vec<Sprint>> {
        let query = [("state", "active,future".to_string()), ("maxResults", "50".to_string())];
        let page: SprintPage =
            self.client.get(&format!("{AGILE}/board/{board}/sprint"), &query).await?;
        Ok(page.values.into_iter().map(|s| Sprint { id: s.id.to_string(), name: s.name }).collect())
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
            ("fields", self.board_fields().await?),
        ];
        let scope = self.scope(id).await?;
        query.extend(jql::board_issues(&scope, since).map(|jql| ("jql", jql)));
        let page: OffsetPage =
            self.client.get(&format!("{AGILE}/board/{id}/issue"), &query).await?;
        Ok(Page { next: offset_page(&page), items: self.issues(&page.issues).await? })
    }

    /// Only the keys, a thousand to a page, in the board's scope.
    async fn board_keys(&self, id: &BoardId) -> Result<Vec<IssueKey>> {
        let scope = self.scope(id).await?;
        let jql = jql::board_issues(&scope, None);
        let mut keys = vec![];
        loop {
            let mut query = vec![
                ("startAt", keys.len().to_string()),
                ("maxResults", KEY_PAGE.to_string()),
                ("fields", "key".to_string()),
            ];
            query.extend(jql.clone().map(|jql| ("jql", jql)));
            let page: KeyPage =
                self.client.get(&format!("{AGILE}/board/{id}/issue"), &query).await?;
            let next = page.start_at + page.issues.len() as u64;
            let last = page.issues.is_empty() || next >= page.total;
            keys.extend(page.issues.into_iter().map(|i| IssueKey::parse(i.key)));
            if last {
                return Ok(keys);
            }
        }
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

/// A page of a board's issues with only their keys.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyPage {
    start_at: u64,
    total: u64,
    issues: Vec<KeyOnly>,
}

#[derive(serde::Deserialize)]
struct KeyOnly {
    key: String,
}

/// The most keys Jira gives in one page of a board.
const KEY_PAGE: u64 = 1000;

/// The most keys Jira's bulkfetch takes in one request.
const BULK_FETCH_MAX: usize = 100;

/// Jira Cloud's bulk read: each issue with its `comment` field and its
/// changelog, read one by one; and the keys it refused.
#[derive(serde::Deserialize)]
struct BulkFetch {
    issues: Vec<serde_json::Value>,
    #[serde(default, rename = "issueErrors")]
    issue_errors: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct BulkIssue {
    #[serde(flatten)]
    issue: IssueDto,
    changelog: Option<Histories>,
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

    /// Every page of the issues whose parent it is, in the board's order.
    async fn children(&self, key: &IssueKey) -> Result<Vec<IssueRef>> {
        let query = Query::Native(format!("parent = {} ORDER BY rank", jql::quote(key.as_str())));
        let mut out = vec![];
        let mut token = None;
        loop {
            let page = self.search_jql(&query, token).await?;
            out.extend(page.items.iter().map(Issue::as_ref));
            match page.next {
                Some(next) => token = Some(next),
                None => return Ok(out),
            }
        }
    }

    /// Cloud only: Data Center has no GraphQL gateway.
    async fn field_layout(&self, key: &IssueKey) -> Result<FieldLayout> {
        if self.settings.flavor == JiraFlavor::DataCenter {
            return Err(DomainError::Unsupported(Capability::CustomFields));
        }
        let cloud = self
            .cloud
            .get_or_try_init(|| async {
                let info: serde_json::Value = self.client.get("/_edge/tenant_info", &[]).await?;
                info["cloudId"].as_str().map(String::from).ok_or_else(|| {
                    DomainError::Transport("the site did not say its cloud id".into())
                })
            })
            .await?;
        let project = key.project().unwrap_or_default();
        let body = json!({
            "operationName": "KolumFieldLayout",
            "query": LAYOUT_QUERY,
            "variables": { "cloudId": cloud, "key": key.as_str(), "project": project },
        });
        let response = self.client.send(Method::POST, "/gateway/api/graphql", &body).await?;
        map::field_layout(&response, &self.ctx().await?.fields)
    }

    async fn comments(&self, key: &IssueKey) -> Result<Vec<Comment>> {
        let query = [("orderBy", "created".to_string()), ("maxResults", PAGE.to_string())];
        let page: CommentPage =
            self.client.get(&format!("{}/issue/{key}/comment", self.api()), &query).await?;
        Ok(page.comments.iter().filter_map(map::comment).collect())
    }

    /// `BULK_FETCH_MAX` at a time, in turns, each handed on as it comes.
    async fn issues_in_full(
        &self,
        keys: &[IssueKey],
        read: &mut (dyn FnMut(Vec<IssueInFull>) + Send),
    ) -> Result<()> {
        let ctx = self.ctx().await?;
        let path = format!("{}/issue/bulkfetch", self.api());
        for batch in keys.chunks(BULK_FETCH_MAX) {
            let body = json!({
                "issueIdsOrKeys": batch.iter().map(IssueKey::as_str).collect::<Vec<_>>(),
                "fields": ["*all"],
                "expand": ["changelog"],
            });
            read(read_bulk(self.client.send(Method::POST, &path, &body).await?, ctx)?);
        }
        Ok(())
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
    async fn editable_fields(&self, key: &IssueKey) -> Result<Vec<EditableField>> {
        let meta: EditMetaDto =
            self.client.get(&format!("{}/issue/{key}/editmeta", self.api()), &[]).await?;
        let ctx = self.ctx().await.ok();
        let order = ctx.map_or(&[][..], |c| &c.priorities);
        Ok(map::editable_fields(&meta, &self.known().await, self.settings.flavor, order))
    }

    async fn transitions(&self, key: &IssueKey) -> Result<Vec<Transition>> {
        let query = [("expand", "transitions.fields".to_string())];
        let page: TransitionPage =
            self.client.get(&format!("{}/issue/{key}/transitions", self.api()), &query).await?;
        let known = self.known().await;
        Ok(page.transitions.iter().map(|t| map::transition(t, &known)).collect())
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
        let body = json!({ self.user_key(): user.map(|u| u.as_str()) });
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
        let mut fields = self.member_fields(&update);
        let ctx = self.ctx().await?;
        let user_key = self.user_key();
        for edit in &update.fields {
            let id = edit.provider_id.to_string();
            let schema = ctx
                .schemas
                .get(&id)
                .ok_or_else(|| DomainError::Transport(format!("Jira lists no field {id}")))?;
            let rich = |t: &RichText| self.rich(t);
            fields.insert(id, map::field_json(schema, edit.value.as_ref(), user_key, &rich));
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
        self.issue(&IssueKey::parse(key)).await
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
        let new = NewIssue {
            project: key.project().unwrap_or_default().into(),
            kind: source.kind,
            title: format!("CLONE - {}", source.title),
            description: source.description.unwrap_or_default(),
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
        Capabilities::new(
            [
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
                ChangedSince,
            ]
            .into_iter()
            // shortcut: Data Center's bulk read is not known; declare it there once one is.
            .chain((self.settings.flavor == JiraFlavor::Cloud).then_some(BulkRead)),
        )
    }
}

/// A bulkfetch's answer, one issue at a time: one of an odd shape, or one
/// Jira refused, costs only itself.
fn read_bulk(answer: serde_json::Value, ctx: &Ctx) -> Result<Vec<IssueInFull>> {
    let read: BulkFetch = serde_json::from_value(answer)
        .map_err(|e| DomainError::Transport(format!("bulkfetch: {e}")))?;
    for refused in &read.issue_errors {
        tracing::info!("not read in bulk: {refused}");
    }
    let one = |issue: serde_json::Value| {
        let bulk: BulkIssue = serde_json::from_value(issue).map_err(|e| e.to_string())?;
        in_full(&bulk, ctx).map_err(|e| format!("{}: {e}", bulk.issue.key))
    };
    let read = read.issues.into_iter().map(one);
    Ok(read
        .filter_map(|r| r.inspect_err(|e| tracing::warn!("left out of a bulk read: {e}")).ok())
        .collect())
}

/// One issue of a bulk read: its comments from its `comment` field, its
/// history from its changelog; the first page of each.
fn in_full(bulk: &BulkIssue, ctx: &Ctx) -> Result<IssueInFull> {
    let key = &bulk.issue.key;
    let comments = bulk.issue.fields.get("comment").and_then(|c| c["comments"].as_array());
    // One by one: a comment of an odd shape costs only itself.
    let comments = comments.into_iter().flatten().filter_map(|c| {
        serde_json::from_value::<CommentDto>(c.clone())
            .inspect_err(|e| tracing::warn!("a comment of {key} left out: {e}"))
            .ok()
    });
    let histories = bulk.changelog.as_ref().map_or(&[][..], |c| &c.histories[..]);
    Ok(IssueInFull {
        issue: map::issue(&bulk.issue, ctx)?,
        comments: comments.filter_map(|c| map::comment(&c)).collect(),
        activity: histories.iter().filter_map(map::activity).collect(),
    })
}

#[cfg(test)]
mod tests;
