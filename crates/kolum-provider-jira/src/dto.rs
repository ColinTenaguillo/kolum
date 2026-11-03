//! Jira's JSON, as it comes. Nothing outside this adapter sees these types:
//! `map` turns them into the domain's.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardPage {
    pub values: Vec<BoardDto>,
    #[serde(default)]
    pub is_last: bool,
}

#[derive(Debug, Deserialize)]
pub struct BoardDto {
    pub id: u64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub location: Option<LocationDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationDto {
    pub project_key: Option<String>,
    pub project_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardConfigDto {
    pub id: u64,
    pub name: String,
    pub column_config: ColumnConfigDto,
}

#[derive(Debug, Deserialize)]
pub struct ColumnConfigDto {
    pub columns: Vec<ColumnDto>,
}

#[derive(Debug, Deserialize)]
pub struct ColumnDto {
    pub name: String,
    pub statuses: Vec<IdDto>,
    pub max: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct IdDto {
    pub id: String,
}

/// `/board/{id}/issue` and v2 `/search`: offset pages.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffsetPage {
    pub issues: Vec<IssueDto>,
    pub start_at: u64,
    pub total: u64,
}

/// v3 `/search/jql`: cursor pages.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenPage {
    pub issues: Vec<IssueDto>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IssueDto {
    pub id: String,
    pub key: String,
    /// Kept loose: which custom fields exist differs per instance, and the
    /// typed fields below are read out of this same map.
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    /// Cloud.
    pub account_id: Option<String>,
    /// Data Center.
    pub name: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub avatar_urls: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub category: Option<CategoryDto>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryDto {
    pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTypeDto {
    pub name: String,
    #[serde(default)]
    pub subtask: bool,
    pub hierarchy_level: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct NamedDto {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RefDto {
    pub key: String,
    pub fields: RefFieldsDto,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefFieldsDto {
    pub summary: String,
    pub status: Option<StatusDto>,
    pub issuetype: Option<IssueTypeDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDto {
    #[serde(rename = "type")]
    pub kind: LinkTypeDto,
    pub inward_issue: Option<RefDto>,
    pub outward_issue: Option<RefDto>,
}

#[derive(Debug, Deserialize)]
pub struct LinkTypeDto {
    pub inward: String,
    pub outward: String,
}

#[derive(Debug, Deserialize)]
pub struct SprintDto {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct CommentPage {
    pub comments: Vec<CommentDto>,
}

#[derive(Debug, Deserialize)]
pub struct CommentDto {
    pub id: String,
    pub author: Option<UserDto>,
    pub body: Value,
    pub created: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoryDto {
    pub author: Option<UserDto>,
    pub created: String,
    pub items: Vec<HistoryItemDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemDto {
    pub field: String,
    pub from_string: Option<String>,
    pub to_string: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FieldDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub custom: bool,
    pub schema: Option<FieldSchemaDto>,
}

#[derive(Debug, Deserialize)]
pub struct FieldSchemaDto {
    #[serde(rename = "type")]
    pub kind: String,
    pub custom: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransitionPage {
    pub transitions: Vec<TransitionDto>,
}

#[derive(Debug, Deserialize)]
pub struct TransitionDto {
    pub id: String,
    pub name: String,
    pub to: StatusDto,
    #[serde(default)]
    pub fields: BTreeMap<String, TransitionFieldDto>,
}

#[derive(Debug, Deserialize)]
pub struct TransitionFieldDto {
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDto {
    pub filename: String,
    /// The file itself, on the Jira site: it needs the credentials.
    pub content: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub size: u64,
}
