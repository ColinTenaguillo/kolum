//! The neutral Query, written as JQL.

use kolum_domain::Filter;
use time::OffsetDateTime;

/// A JQL string literal: always quoted, so a value with a space, a dash or
/// a reserved word (`"In Review"`, `"PROJ-10"`, `"order"`) stays a value.
pub fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn any_of(field: &str, values: &[String]) -> Option<String> {
    match values {
        [] => None,
        [one] => Some(format!("{field} = {}", quote(one))),
        many => Some(format!(
            "{field} in ({})",
            many.iter().map(|v| quote(v)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// `assignees` are the ids a `@name` resolved to; `@me` needs none.
pub fn filter(f: &Filter, assignees: &[String]) -> String {
    use kolum_domain::UserSel;
    let mut clauses: Vec<String> = vec![];
    if let Some(text) = &f.text {
        clauses.push(format!("text ~ {}", quote(text)));
    }
    match &f.assignee {
        Some(UserSel::Me) => clauses.push("assignee = currentUser()".into()),
        Some(UserSel::Named(_)) => clauses.extend(any_of("assignee", assignees)),
        None => {}
    }
    clauses.extend(any_of("status", &f.statuses));
    clauses.extend(any_of("issuetype", &f.types));
    clauses.extend(any_of("labels", &f.labels));
    if let Some(epic) = &f.epic {
        clauses.push(format!("parent = {}", quote(epic)));
    }
    if let Some(sprint) = &f.sprint {
        // A number is a sprint id; anything else its name.
        let value = if sprint.parse::<u64>().is_ok() { sprint.clone() } else { quote(sprint) };
        clauses.push(format!("sprint = {value}"));
    }
    with_order(clauses.join(" AND "))
}

pub fn with_order(jql: String) -> String {
    if jql.is_empty() {
        "ORDER BY updated DESC".into()
    } else {
        format!("{jql} ORDER BY updated DESC")
    }
}

/// The clause for an incremental sync. Minutes are JQL's finest grain, so
/// this rounds down and may return an issue twice; the merge is by key.
pub fn updated_since(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    format!("updated >= \"{}\"", t.format(&format).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::UserSel;

    #[test]
    fn values_are_quoted_and_escaped() {
        assert_eq!(quote("In Review"), "\"In Review\"");
        assert_eq!(quote(r#"say "hi" \o/"#), r#""say \"hi\" \\o/""#);
    }

    #[test]
    fn an_empty_filter_is_just_an_order() {
        assert_eq!(filter(&Filter::default(), &[]), "ORDER BY updated DESC");
    }

    #[test]
    fn every_field_becomes_a_clause() {
        let f = Filter {
            text: Some("login timeout".into()),
            assignee: Some(UserSel::Me),
            statuses: vec!["In Review".into()],
            types: vec!["Bug".into(), "Story".into()],
            labels: vec!["backend".into()],
            epic: Some("PROJ-10".into()),
            sprint: Some("14".into()),
        };
        assert_eq!(
            filter(&f, &[]),
            "text ~ \"login timeout\" AND assignee = currentUser() AND status = \"In Review\" \
             AND issuetype in (\"Bug\", \"Story\") AND labels = \"backend\" AND parent = \"PROJ-10\" \
             AND sprint = 14 ORDER BY updated DESC"
        );
    }

    #[test]
    fn a_named_assignee_uses_the_resolved_ids_and_a_sprint_name_is_quoted() {
        let f = Filter {
            assignee: Some(UserSel::Named("ana".into())),
            sprint: Some("Sprint 14".into()),
            ..Filter::default()
        };
        assert_eq!(
            filter(&f, &["5b10a2844c20165700ede21g".into(), "abc".into()]),
            "assignee in (\"5b10a2844c20165700ede21g\", \"abc\") AND sprint = \"Sprint 14\" ORDER BY updated DESC"
        );
    }

    #[test]
    fn the_incremental_clause_is_in_utc_minutes() {
        let t = time::macros::datetime!(2026-09-01 11:07:59 +02:00);
        assert_eq!(updated_since(t), "updated >= \"2026-09-01 09:07\"");
    }
}
