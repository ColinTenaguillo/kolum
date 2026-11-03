//! The Jira adapter: Cloud's REST v3 and Agile 1.0 APIs, with the paths and
//! authentication switching to v2 and a Bearer token on Data Center.
//!
//! Read-only for now: writes answer `Unsupported(Write)`, and
//! `Capabilities` says so up front, so the UI does not offer them.

mod adf;
mod client;
mod dto;
pub mod jql;
mod map;
mod settings;

pub use settings::{JiraFlavor, JiraSettings};

use async_trait::async_trait;
use client::Client;
use dto::*;
use kolum_domain::*;
use map::Ctx;
use time::OffsetDateTime;
use tokio::sync::OnceCell;

const AGILE: &str = "/rest/agile/1.0";
const PAGE: u64 = 100;
const SEARCH_PAGE: u64 = 50;
/// What a card, the picker and the detail need. Custom fields are added
/// once known; `*all` would also bring every plugin's fields on every card.
const FIELDS: &str = "summary,issuetype,status,priority,assignee,reporter,labels,parent,subtasks,\
                      issuelinks,created,updated,duedate,description,timeoriginalestimate";

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
        let settings =
            settings::parse(table, std::env::var("KOLUM_TOKEN").ok(), settings::run_command)?;
        Ok(Self::new(settings))
    }

    /// The board the profile names, if it names one.
    pub fn board_id(&self) -> Option<BoardId> {
        self.settings.board.map(|b| BoardId::new(b.to_string()))
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

    /// `@name` resolved to account ids, for JQL.
    async fn assignee_ids(&self, query: &Query) -> Result<Vec<String>> {
        match query {
            Query::Filter(Filter { assignee: Some(UserSel::Named(name)), .. }) => {
                Ok(self.find_users(name).await?.into_iter().map(|u| u.id.0).collect())
            }
            _ => Ok(vec![]),
        }
    }
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
    async fn search(&self, query: &Query, token: Option<PageToken>) -> Result<Page<Issue>> {
        let jql = match query {
            Query::Native(jql) => jql.clone(),
            Query::Filter(filter) => {
                let ids = self.assignee_ids(query).await?;
                if matches!(filter.assignee, Some(UserSel::Named(_))) && ids.is_empty() {
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

    async fn transition(&self, _: &IssueKey, _: &TransitionId) -> Result<()> {
        Err(DomainError::Unsupported(Capability::Write))
    }

    async fn assign(&self, _: &IssueKey, _: Option<&UserId>) -> Result<()> {
        Err(DomainError::Unsupported(Capability::Write))
    }

    async fn comment(&self, _: &IssueKey, _: RichText) -> Result<Comment> {
        Err(DomainError::Unsupported(Capability::Write))
    }

    async fn update(&self, _: &IssueKey, _: IssueUpdate) -> Result<()> {
        Err(DomainError::Unsupported(Capability::Write))
    }

    async fn create(&self, _: NewIssue) -> Result<Issue> {
        Err(DomainError::Unsupported(Capability::Write))
    }

    async fn clone_issue(&self, _: &IssueKey) -> Result<Issue> {
        Err(DomainError::Unsupported(Capability::Write))
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
        ])
    }
}

#[cfg(test)]
mod tests;
