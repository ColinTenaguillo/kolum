//! Jira's shapes to the domain's: the anti-corruption layer. Every Jira
//! word (statusCategory, hierarchyLevel, customfield_10020) stops here.

use crate::dto::*;
use crate::settings::JiraFlavor;
use crate::{adf, wiki};
use kolum_domain::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// Which custom fields hold the estimate, the sprint and the old epic
/// link. Their ids differ on every instance: found from `/field`, unless
/// the profile names them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldIds {
    pub estimate: Option<String>,
    pub sprint: Option<String>,
    pub epic_link: Option<String>,
}

/// What mapping an issue needs beyond the issue.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub base_url: String,
    pub fields: FieldIds,
    /// Custom field names by id, for the detail's right-hand column.
    pub names: BTreeMap<String, String>,
    /// Every field's schema by id: how it takes a value.
    pub schemas: BTreeMap<String, FieldSchemaDto>,
}

const ESTIMATE_NAMES: &[&str] = &["Story Points", "Story point estimate"];

/// Reads `/field`, then lets the profile's choices win.
pub fn detect_fields(fields: &[FieldDto], forced: &FieldIds) -> FieldIds {
    let by_schema = |suffix: &str| {
        fields
            .iter()
            .find(|f| {
                f.schema
                    .as_ref()
                    .and_then(|s| s.custom.as_deref())
                    .is_some_and(|c| c.ends_with(suffix))
            })
            .map(|f| f.id.clone())
    };
    let estimate = fields
        .iter()
        .find(|f| {
            f.custom
                && ESTIMATE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(&f.name))
                && f.schema.as_ref().is_some_and(|s| s.kind == "number")
        })
        .map(|f| f.id.clone());
    FieldIds {
        estimate: forced.estimate.clone().or(estimate),
        sprint: forced.sprint.clone().or_else(|| by_schema(":gh-sprint")),
        epic_link: forced.epic_link.clone().or_else(|| by_schema(":gh-epic-link")),
    }
}

/// How Jira takes `value` in a field of this schema: an option as
/// `{"value"}`, a version or a priority as `{"name"}`, a user by
/// `user_key`, rich text as `rich` writes it. `None` empties the field.
pub fn field_json(
    schema: &FieldSchemaDto,
    value: Option<&FieldValue>,
    user_key: &str,
    rich: &dyn Fn(&RichText) -> Value,
) -> Value {
    let Some(value) = value else { return empty(schema) };
    match value {
        FieldValue::Text(t) => one(&schema.kind, t),
        FieldValue::Options(all) => {
            let item = schema.items.as_deref().unwrap_or("string");
            Value::Array(all.iter().map(|v| one(item, v)).collect())
        }
        // A count of points: whole numbers stay whole.
        FieldValue::Number(n) => whole(*n),
        FieldValue::Date(d) => json!(d.to_string()),
        FieldValue::User(u) => json!({ user_key: u.id.as_str() }),
        FieldValue::RichText(t) => rich(t),
        // A sprint by its number.
        FieldValue::Sprint(s) => s.id.parse().map_or_else(|_| json!(s.id), whole),
        FieldValue::Issue(i) => one(&schema.kind, i.key.as_str()),
    }
}

/// An emptied field: a list takes none, but a sprint is left by null.
fn empty(schema: &FieldSchemaDto) -> Value {
    let sprint = schema.custom.as_deref().is_some_and(|c| c.ends_with(":gh-sprint"));
    if schema.kind == "array" && !sprint { json!([]) } else { Value::Null }
}

/// One value of a field of this kind, by what Jira names it by.
fn one(kind: &str, v: &str) -> Value {
    match kind {
        "option" => json!({ "value": v }),
        "priority" | "version" | "component" => json!({ "name": v }),
        "issuelink" => json!({ "key": v }),
        _ => json!(v),
    }
}

fn whole(n: f64) -> Value {
    if n.fract() == 0.0 { json!(n as i64) } else { json!(n) }
}

/// The web issue view's layout, from the GraphQL answer to `LAYOUT_QUERY`:
/// the pinned fields, the details (shown even empty) and the others, shown
/// once filled, beside the issue; the body's fields in their order.
/// Fields kolum has a name for go by that name, as `detail_fields` does.
pub fn field_layout(response: &Value, known: &FieldIds) -> Result<FieldLayout> {
    if let Some(message) = response.pointer("/errors/0/message").and_then(Value::as_str) {
        return Err(DomainError::Transport(message.to_string()));
    }
    let jira = &response["data"]["jira"];
    let pinned: Vec<FieldId> = jira["userPreferences"]["issueViewPinnedFields"]
        .as_str()
        .unwrap_or("")
        .split(|c: char| c == ',' || c == '"' || c == '[' || c == ']' || c.is_whitespace())
        .filter(|id| !id.is_empty())
        .map(FieldId::new)
        .collect();
    let containers = jira["issueByKey"]["fieldsByContainerTypes"]["containerFields"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let nodes = |kind: &str| -> Vec<(FieldId, String, String)> {
        let found = containers.iter().find(|c| c["containerType"] == kind);
        let edges = found.and_then(|c| c["fields"]["edges"].as_array());
        edges
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let node = &e["node"];
                let id = FieldId::new(node["fieldId"].as_str()?);
                let text = |key: &str| node[key].as_str().unwrap_or("").to_string();
                Some((id, text("type"), text("name")))
            })
            .collect()
    };
    let (primary, secondary) = (nodes("PRIMARY"), nodes("SECONDARY"));
    // A pinned field is named where the layout lists it.
    let name_of = |id: &FieldId| {
        let listed = primary.iter().chain(&secondary).find(|n| &n.0 == id);
        listed.map_or_else(|| id.to_string(), |n| n.2.clone())
    };
    let mut side: Vec<LayoutField> = vec![];
    let mut add = |id: &FieldId, name: String, only_filled: bool| {
        let id = domain_id(id.as_str(), known);
        if !side.iter().any(|f| f.id == id) {
            side.push(LayoutField { id, name, only_filled });
        }
    };
    pinned.iter().for_each(|id| add(id, name_of(id), false));
    for (id, kind, name) in &primary {
        // The web hides the old Advanced Roadmaps Parent Link while it is
        // empty, though the layout lists it; seen, not documented.
        add(id, name.clone(), kind == "com.atlassian.jpo:jpo-custom-field-parent");
    }
    secondary.iter().for_each(|(id, _, name)| add(id, name.clone(), true));
    let body = nodes("CONTENT").into_iter().map(|n| n.0).collect();
    Ok(FieldLayout { side, body })
}

/// Jira's system fields the domain names otherwise, with the domain's
/// name; the others (`description`, `labels`, `assignee`) read the same.
const NATIVE: [(&str, &str); 3] =
    [("summary", FieldId::TITLE), ("duedate", FieldId::DUE), ("parent", FieldId::EPIC)];

/// A Jira field by the domain's id: the system fields above, and the
/// custom ones the issue reads into its members (the story points, the
/// sprint, the old Epic Link, which says the same as Parent).
pub fn domain_id(jira: &str, known: &FieldIds) -> FieldId {
    let is = |field: &Option<String>| field.as_deref() == Some(jira);
    let native = NATIVE.iter().find(|(j, _)| *j == jira).map(|(_, domain)| *domain);
    FieldId::new(match native {
        _ if is(&known.estimate) => FieldId::ESTIMATE,
        _ if is(&known.sprint) => FieldId::SPRINT,
        _ if is(&known.epic_link) => FieldId::EPIC,
        native => native.unwrap_or(jira),
    })
}

/// The fields editmeta lets kolum set, by name, each with how it is
/// edited. What kolum cannot edit (attachments, a date and time) is left
/// out, and so is the one of Parent and Epic Link that says the same as
/// the other: Cloud sets an epic by Parent, Data Center by Epic Link.
pub fn editable_fields(
    meta: &EditMetaDto,
    known: &FieldIds,
    flavor: JiraFlavor,
) -> Vec<EditableField> {
    let offered = |id: Option<&str>| id.is_some_and(|id| meta.fields.contains_key(id));
    let (epic_link, parent) = (known.epic_link.as_deref(), Some("parent"));
    let other = match flavor {
        JiraFlavor::Cloud => offered(parent).then_some(epic_link).flatten(),
        JiraFlavor::DataCenter => offered(epic_link).then_some(parent).flatten(),
    };
    let mut out: Vec<EditableField> = meta
        .fields
        .iter()
        .filter(|(id, _)| Some(id.as_str()) != other)
        .filter_map(|(id, f)| {
            let kolum = domain_id(id, known);
            let member = FieldId::members().any(|(m, _)| m == kolum.as_str());
            Some(EditableField {
                provider_id: FieldId::new(id),
                on_issue: member || read_as_field(id),
                id: kolum,
                name: f.name.clone(),
                kind: edit_kind(id, f)?,
                required: f.required,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn edit_kind(id: &str, f: &EditMetaFieldDto) -> Option<FieldKind> {
    if !f.operations.iter().any(|o| o == "set") {
        return None;
    }
    let schema = f.schema.as_ref()?;
    let custom = schema.custom.as_deref().unwrap_or("");
    let options: Vec<String> = f
        .allowed_values
        .iter()
        .filter_map(|v| v.value.clone().or_else(|| v.name.clone()))
        .collect();
    Some(match (schema.kind.as_str(), schema.items.as_deref()) {
        _ if custom.ends_with(":textarea") || matches!(id, "description" | "environment") => {
            FieldKind::RichText
        }
        _ if custom.ends_with(":gh-sprint") => FieldKind::Sprint,
        _ if custom.ends_with(":gh-epic-link") => FieldKind::Issue,
        ("issuelink", _) => FieldKind::Issue,
        ("string", _) => FieldKind::Text,
        ("number", _) => FieldKind::Number,
        ("date", _) => FieldKind::Date,
        ("user", _) => FieldKind::User,
        ("array", Some("string")) => FieldKind::Words,
        ("option" | "priority" | "version" | "component", _) if !options.is_empty() => {
            FieldKind::Choice { options, many: false }
        }
        ("array", Some("option" | "version" | "component")) if !options.is_empty() => {
            FieldKind::Choice { options, many: true }
        }
        _ => return None,
    })
}

pub fn schemas(fields: &[FieldDto]) -> BTreeMap<String, FieldSchemaDto> {
    fields.iter().filter_map(|f| Some((f.id.clone(), f.schema.clone()?))).collect()
}

/// System fields the issue holds as a list of names, read like custom ones
/// since the domain has no member for them.
const SYSTEM_LISTS: [&str; 2] = ["fixVersions", "components"];

/// A field the issue is read for into its custom fields.
fn read_as_field(id: &str) -> bool {
    id.starts_with("customfield_") || SYSTEM_LISTS.contains(&id)
}

pub fn custom_names(fields: &[FieldDto]) -> BTreeMap<String, String> {
    let read = |f: &&FieldDto| f.custom || SYSTEM_LISTS.contains(&f.id.as_str());
    fields.iter().filter(read).map(|f| (f.id.clone(), f.name.clone())).collect()
}

fn get<T: DeserializeOwned>(fields: &BTreeMap<String, Value>, name: &str) -> Option<T> {
    fields.get(name).filter(|v| !v.is_null()).and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn bad(what: &str) -> DomainError {
    DomainError::Transport(format!("Jira sent an issue without a readable {what}"))
}

/// Jira's timestamps: `2026-09-01T09:00:00.000+0000`, an offset with no
/// colon, which RFC 3339 does not allow.
pub fn datetime(s: &str) -> Option<OffsetDateTime> {
    let jira = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond][offset_hour sign:mandatory][offset_minute]"
    );
    OffsetDateTime::parse(s, jira)
        .or_else(|_| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339))
        .ok()
}

pub fn user(dto: &UserDto) -> User {
    let id = dto.account_id.clone().or_else(|| dto.name.clone()).unwrap_or_default();
    User {
        id: UserId::new(id),
        display_name: dto.display_name.clone(),
        avatar_url: dto.avatar_urls.get("48x48").cloned(),
    }
}

pub fn status(dto: &StatusDto) -> Status {
    let category = match dto.category.as_ref().map(|c| c.key.as_str()) {
        Some("indeterminate") => StatusCategory::Doing,
        Some("done") => StatusCategory::Done,
        _ => StatusCategory::Todo,
    };
    Status { id: StatusId::new(&dto.id), name: dto.name.clone(), category }
}

pub fn kind(dto: &IssueTypeDto) -> IssueKind {
    let name = dto.name.to_lowercase();
    if dto.subtask {
        IssueKind::Subtask
    } else if dto.hierarchy_level == Some(1) || name == "epic" {
        IssueKind::Epic
    } else if name.contains("bug") || name.contains("defect") {
        IssueKind::Bug
    } else if name.contains("story") {
        IssueKind::Story
    } else if name.contains("task") {
        IssueKind::Task
    } else {
        IssueKind::Other
    }
}

/// Jira's default scheme, and the older one (Blocker…Trivial).
pub fn priority(name: &str) -> Option<Priority> {
    Some(match name.to_lowercase().as_str() {
        "highest" | "blocker" | "critical" | "urgent" => Priority::Highest,
        "high" | "major" => Priority::High,
        "medium" | "normal" => Priority::Medium,
        "low" | "minor" => Priority::Low,
        "lowest" | "trivial" => Priority::Lowest,
        _ => return None,
    })
}

fn issue_ref(dto: &RefDto) -> IssueRef {
    IssueRef {
        key: IssueKey::parse(&dto.key),
        title: dto.fields.summary.clone(),
        kind: dto.fields.issuetype.as_ref().map_or(IssueKind::Other, kind),
        status: dto.fields.status.as_ref().map(status),
    }
}

fn link(dto: &LinkDto) -> Option<IssueLink> {
    match (&dto.inward_issue, &dto.outward_issue) {
        (Some(other), _) => {
            Some(IssueLink { relation: dto.kind.inward.clone(), issue: issue_ref(other) })
        }
        (_, Some(other)) => {
            Some(IssueLink { relation: dto.kind.outward.clone(), issue: issue_ref(other) })
        }
        _ => None,
    }
}

/// Cloud sends sprints as objects; Data Center as strings like
/// `com.atlassian.greenhopper.service.sprint.Sprint@1f[id=3,state=ACTIVE,name=Sprint 3,...]`.
/// The active one wins, else the latest.
fn sprint(value: &Value) -> Option<Sprint> {
    let all: Vec<(Sprint, bool)> = value
        .as_array()?
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => {
                let field = |k: &str| {
                    s.split([',', '[', ']']).find_map(|p| p.strip_prefix(k)).map(String::from)
                };
                let state = field("state=").unwrap_or_default();
                Some((
                    Sprint { id: field("id=")?, name: field("name=")? },
                    state.eq_ignore_ascii_case("active"),
                ))
            }
            other => {
                let dto: SprintDto = serde_json::from_value(other.clone()).ok()?;
                Some((Sprint { id: dto.id.to_string(), name: dto.name }, dto.state == "active"))
            }
        })
        .collect();
    all.iter().find(|(_, active)| *active).or(all.last()).map(|(s, _)| s.clone())
}

fn description(value: Option<&Value>) -> RichText {
    match value {
        Some(Value::String(text)) => wiki::to_rich_text(text),
        Some(doc @ Value::Object(_)) => adf::to_rich_text(doc),
        _ => RichText::default(),
    }
}

fn field_value(value: &Value) -> Option<FieldValue> {
    Some(match value {
        Value::String(s) => FieldValue::Text(s.clone()),
        Value::Number(n) => FieldValue::Number(n.as_f64()?),
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("doc") => {
            FieldValue::RichText(adf::to_rich_text(value))
        }
        Value::Object(o) if o.contains_key("displayName") => {
            FieldValue::User(user(&serde_json::from_value(value.clone()).ok()?))
        }
        Value::Object(o) => {
            FieldValue::Text(o.get("value").or(o.get("name"))?.as_str()?.to_string())
        }
        Value::Array(items) => FieldValue::Options(
            items
                .iter()
                .filter_map(|i| match field_value(i)? {
                    FieldValue::Text(t) => Some(t),
                    _ => None,
                })
                .collect(),
        ),
        _ => return None,
    })
}

pub fn issue(dto: &IssueDto, ctx: &Ctx) -> Result<Issue> {
    let f = &dto.fields;
    let issuetype: IssueTypeDto = get(f, "issuetype").ok_or_else(|| bad("type"))?;
    let status_dto: StatusDto = get(f, "status").ok_or_else(|| bad("status"))?;
    let created: String = get(f, "created").ok_or_else(|| bad("creation date"))?;
    let updated: Option<String> = get(f, "updated");
    let created = datetime(&created).ok_or_else(|| bad("creation date"))?;
    let parent = get::<RefDto>(f, "parent").map(|p| issue_ref(&p)).or_else(|| {
        let key: String = get(f, ctx.fields.epic_link.as_deref()?)?;
        Some(IssueRef {
            key: IssueKey::parse(key),
            title: String::new(),
            kind: IssueKind::Epic,
            status: None,
        })
    });
    let estimate = ctx
        .fields
        .estimate
        .as_deref()
        .and_then(|id| get::<f64>(f, id))
        .map(Estimate::Points)
        .or_else(|| get::<u64>(f, "timeoriginalestimate").map(Estimate::Seconds));
    let known = [&ctx.fields.estimate, &ctx.fields.sprint, &ctx.fields.epic_link];
    let fields = f
        .iter()
        .filter(|(id, _)| {
            read_as_field(id) && !known.iter().any(|k| k.as_deref() == Some(id.as_str()))
        })
        .filter_map(|(id, v)| {
            let value = field_value(v)?;
            let name = ctx.names.get(id).cloned().unwrap_or_else(|| id.clone());
            Some(CustomField { id: FieldId::new(id), name, value })
        })
        .collect();
    Ok(Issue {
        id: IssueId::new(&dto.id),
        key: IssueKey::parse(&dto.key),
        title: get(f, "summary").unwrap_or_default(),
        kind: IssueType { kind: kind(&issuetype), name: issuetype.name },
        status: status(&status_dto),
        priority: get::<NamedDto>(f, "priority").and_then(|p| priority(&p.name)),
        assignee: get::<UserDto>(f, "assignee").map(|u| user(&u)),
        reporter: get::<UserDto>(f, "reporter").map(|u| user(&u)),
        labels: get(f, "labels").unwrap_or_default(),
        parent,
        subtasks: get::<Vec<RefDto>>(f, "subtasks")
            .unwrap_or_default()
            .iter()
            .map(issue_ref)
            .collect(),
        links: get::<Vec<LinkDto>>(f, "issuelinks")
            .unwrap_or_default()
            .iter()
            .filter_map(link)
            .collect(),
        estimate,
        sprint: ctx.fields.sprint.as_deref().and_then(|id| f.get(id)).and_then(sprint),
        created,
        updated: updated.as_deref().and_then(datetime).unwrap_or(created),
        due: get::<String>(f, "duedate").and_then(|d| {
            time::Date::parse(&d, time::macros::format_description!("[year]-[month]-[day]")).ok()
        }),
        description: description(f.get("description")),
        fields,
        url: Some(format!("{}/browse/{}", ctx.base_url, dto.key)),
        attachments: get::<Vec<AttachmentDto>>(f, "attachment")
            .unwrap_or_default()
            .into_iter()
            .map(|a| Attachment {
                name: a.filename,
                url: a.content,
                mime: a.mime_type,
                size: a.size,
            })
            .collect(),
    })
}

pub fn board(summary: &BoardDto, config: &BoardConfigDto) -> Board {
    Board {
        id: BoardId::new(config.id.to_string()),
        name: config.name.clone(),
        kind: board_kind(&summary.kind),
        project: project(summary),
        columns: config
            .column_config
            .columns
            .iter()
            .map(|c| Column {
                name: c.name.clone(),
                statuses: c.statuses.iter().map(|s| StatusId::new(&s.id)).collect(),
                wip_limit: c.max,
            })
            .collect(),
    }
}

pub fn board_kind(kind: &str) -> BoardKind {
    match kind {
        "kanban" => BoardKind::Kanban,
        "scrum" => BoardKind::Scrum,
        _ => BoardKind::Other,
    }
}

pub fn project(dto: &BoardDto) -> Option<Project> {
    let location = dto.location.as_ref()?;
    Some(Project {
        key: location.project_key.clone()?,
        name: location.project_name.clone().unwrap_or_default(),
    })
}

pub fn summary(dto: &BoardDto) -> BoardSummary {
    BoardSummary {
        id: BoardId::new(dto.id.to_string()),
        name: dto.name.clone(),
        kind: board_kind(&dto.kind),
        project: project(dto),
    }
}

fn someone() -> User {
    User { id: UserId::new(""), display_name: "Someone".into(), avatar_url: None }
}

pub fn comment(dto: &CommentDto) -> Option<Comment> {
    Some(Comment {
        id: dto.id.clone(),
        author: dto.author.as_ref().map_or_else(someone, user),
        body: description(Some(&dto.body)),
        created: datetime(&dto.created)?,
    })
}

pub fn activity(dto: &HistoryDto) -> Option<ActivityEntry> {
    Some(ActivityEntry {
        author: dto.author.as_ref().map_or_else(someone, user),
        at: datetime(&dto.created)?,
        changes: dto
            .items
            .iter()
            .map(|i| FieldChange {
                field: i.field.clone(),
                from: i.from_string.clone(),
                to: i.to_string.clone(),
            })
            .collect(),
    })
}

pub fn transition(dto: &TransitionDto, known: &FieldIds) -> Transition {
    Transition {
        id: TransitionId::new(&dto.id),
        name: dto.name.clone(),
        to: status(&dto.to),
        required_fields: dto
            .fields
            .iter()
            .filter(|(_, f)| f.required)
            .map(|(id, _)| domain_id(id, known))
            .collect(),
    }
}
