//! The neutral Query, written as JQL.

use kolum_domain::{Filter, IssueKey};
use time::OffsetDateTime;

/// `proj-12983` read as the key `PROJ-12983`: typed text, trimmed and in
/// capitals, when it reads as a Jira key (not GitHub's `repo#12`).
pub fn issue_key(text: &str) -> Option<IssueKey> {
    let key = IssueKey::parse(text.trim().to_uppercase());
    (key.project().is_some() && !key.as_str().contains('#')).then_some(key)
}

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

/// `assignees` are the ids every `@name` resolved to; `@me` and `@none`
/// need none.
pub fn filter(f: &Filter, assignees: &[String]) -> String {
    let mut clauses: Vec<String> = vec![];
    clauses.extend(f.text.as_ref().map(|text| format!("text ~ {}", quote(text))));
    clauses.extend(assignee(&f.assignees, assignees));
    clauses.extend(any_of("status", &f.statuses));
    clauses.extend(any_of("issuetype", &f.types));
    clauses.extend(any_of("labels", &f.labels));
    clauses.extend(f.epic.as_ref().map(|epic| format!("parent = {}", quote(epic))));
    clauses.extend(f.sprint.as_ref().map(|sprint| format!("sprint = {}", sprint_value(sprint))));
    with_order(clauses.join(" AND "))
}

/// One clause for every way of naming the assignee, ORed: they are
/// alternatives, like the values of any field.
fn assignee(sels: &[kolum_domain::UserSel], ids: &[String]) -> Option<String> {
    use kolum_domain::UserSel;
    let mut who: Vec<String> = vec![];
    for sel in sels {
        match sel {
            UserSel::Me => who.push("assignee = currentUser()".into()),
            UserSel::Nobody => who.push("assignee is EMPTY".into()),
            UserSel::Named(_) => {}
        }
    }
    if sels.iter().any(|s| matches!(s, UserSel::Named(_))) {
        who.extend(any_of("assignee", ids));
    }
    match who.len() {
        0 => None,
        1 => who.pop(),
        _ => Some(format!("({})", who.join(" OR "))),
    }
}

/// A number is a sprint id; anything else its name.
fn sprint_value(sprint: &str) -> String {
    if sprint.parse::<u64>().is_ok() { sprint.to_string() } else { quote(sprint) }
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

/// What a board's web view shows of its filter.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub sub_query: Option<String>,
    /// "Hide completed issues older than", as Jira gives it: `-1w`.
    pub done_cutoff: Option<String>,
    /// The statuses of the last column, the only one the cutoff empties.
    pub done_statuses: Vec<String>,
}

/// The JQL of a page of board issues, as its web board shows them: its
/// sub-filter, without the last column's issues resolved before the cutoff,
/// then the incremental clause.
pub fn board_issues(scope: &Scope, since: Option<OffsetDateTime>) -> Option<String> {
    let sub = scope.sub_query.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let sub = sub.map(|q| format!("({q})"));
    let ids: Vec<&str> = scope
        .done_statuses
        .iter()
        .map(String::as_str)
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let elsewhere = if ids.is_empty() {
        String::new()
    } else {
        format!("status not in ({}) OR ", ids.join(", "))
    };
    let cutoff = scope
        .done_cutoff
        .as_deref()
        .filter(|c| is_relative_date(c))
        // In whole days, as the web board counts them.
        .map(|c| format!("({elsewhere}resolved is EMPTY OR resolved >= startOfDay({c}))"));
    let parts: Vec<String> =
        sub.into_iter().chain(cutoff).chain(since.map(updated_since)).collect();
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// `-1w`, `-12d`: what the cutoff holds, and nothing else goes into JQL.
fn is_relative_date(s: &str) -> bool {
    s.strip_prefix('-')
        .and_then(|n| n.strip_suffix(['d', 'w']))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::UserSel;

    #[test]
    fn an_issue_key_is_told_from_text() {
        assert_eq!(issue_key(" proj-12983 "), Some(IssueKey::parse("PROJ-12983")));
        assert_eq!(issue_key("MY_P2-7"), Some(IssueKey::parse("MY_P2-7")));
        assert_eq!(issue_key("my_p2-7").unwrap().project(), Some("MY_P2"));
        for text in
            ["login", "PROJ-", "-12", "2FA-1", "PROJ-12a", "PROJ-1 timeout", "PROJ-1-2", "web#12"]
        {
            assert_eq!(issue_key(text), None, "{text}");
        }
    }

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
            assignees: vec![UserSel::Me],
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
            assignees: vec![UserSel::Named("ana".into())],
            sprint: Some("Sprint 14".into()),
            ..Filter::default()
        };
        assert_eq!(
            filter(&f, &["5b10a2844c20165700ede21g".into(), "abc".into()]),
            "assignee in (\"5b10a2844c20165700ede21g\", \"abc\") AND sprint = \"Sprint 14\" ORDER BY updated DESC"
        );
    }

    #[test]
    fn several_people_are_one_or_group() {
        let f = Filter {
            assignees: vec![UserSel::Me, UserSel::Named("ana".into()), UserSel::Nobody],
            ..Filter::default()
        };
        assert_eq!(
            filter(&f, &["acc-ana".into()]),
            "(assignee = currentUser() OR assignee is EMPTY OR assignee = \"acc-ana\") ORDER BY updated DESC"
        );
        let nobody = Filter { assignees: vec![UserSel::Nobody], ..Filter::default() };
        assert_eq!(filter(&nobody, &[]), "assignee is EMPTY ORDER BY updated DESC");
    }

    #[test]
    fn the_incremental_clause_is_in_utc_minutes() {
        let t = time::macros::datetime!(2026-09-01 11:07:59 +02:00);
        assert_eq!(updated_since(t), "updated >= \"2026-09-01 09:07\"");
    }

    fn scope(sub: Option<&str>, cutoff: Option<&str>, done: &[&str]) -> Scope {
        Scope {
            sub_query: sub.map(String::from),
            done_cutoff: cutoff.map(String::from),
            done_statuses: done.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn the_board_query_is_its_sub_filter_and_the_incremental_clause() {
        let t = time::macros::datetime!(2026-09-01 9:07 UTC);
        let sub = scope(Some("resolved >= -2w OR resolved is EMPTY"), None, &[]);
        assert_eq!(
            board_issues(&sub, None).as_deref(),
            Some("(resolved >= -2w OR resolved is EMPTY)")
        );
        assert_eq!(
            board_issues(&sub, Some(t)).as_deref(),
            Some("(resolved >= -2w OR resolved is EMPTY) AND updated >= \"2026-09-01 09:07\"")
        );
        assert_eq!(
            board_issues(&scope(None, None, &[]), Some(t)).as_deref(),
            Some("updated >= \"2026-09-01 09:07\"")
        );
        let blank = scope(Some("  "), None, &[]);
        assert_eq!(board_issues(&blank, None), None, "a blank sub-filter is none");
    }

    /// As the web board: only the last column's completed issues are hidden.
    #[test]
    fn completed_issues_resolved_before_the_cutoff_leave_the_last_column() {
        assert_eq!(
            board_issues(&scope(Some("project = KOA"), Some("-1w"), &["10403", "10505"]), None)
                .as_deref(),
            Some(
                "(project = KOA) AND (status not in (10403, 10505) OR resolved is EMPTY \
                 OR resolved >= startOfDay(-1w))"
            )
        );
        assert_eq!(
            board_issues(&scope(None, Some("-12d"), &[]), None).as_deref(),
            Some("(resolved is EMPTY OR resolved >= startOfDay(-12d))"),
            "no column known: every completed issue"
        );
        assert_eq!(
            board_issues(&scope(None, Some("-1w"), &["10403", "x) OR (1=1"]), None).as_deref(),
            Some("(status not in (10403) OR resolved is EMPTY OR resolved >= startOfDay(-1w))"),
            "a status id is digits"
        );
        for odd in ["", "-", "1w", "-1y", "-1w OR 1=1", "-w"] {
            let odd = scope(None, Some(odd), &["1"]);
            assert_eq!(board_issues(&odd, None), None, "{odd:?} is no cutoff");
        }
    }
}
