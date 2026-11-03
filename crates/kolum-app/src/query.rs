//! What a person types in the picker, read into a neutral [`Query`].

use kolum_domain::{Filter, Query, UserSel};

/// `@me` (or `@moi`), `@name` and `@none` for the assignee (several are
/// alternatives), `#bug` for the type,
/// `s:review` status, `l:backend` label, `e:PROJ-10` epic, `sp:14` sprint.
/// The rest is free text. A leading `!` sends everything after it to the
/// provider as written (JQL on Jira).
pub fn parse_query(input: &str) -> Query {
    if let Some(native) = input.trim_start().strip_prefix('!') {
        return Query::Native(native.trim().to_string());
    }
    let mut filter = Filter::default();
    let mut text = Vec::new();
    for word in input.split_whitespace() {
        if !apply_prefix(&mut filter, word) {
            text.push(word);
        }
    }
    filter.text = (!text.is_empty()).then(|| text.join(" "));
    Query::Filter(filter)
}

/// False when the word is not a prefix, or a prefix with nothing after it:
/// a lone `#` while typing is text until it says what it filters.
fn apply_prefix(filter: &mut Filter, word: &str) -> bool {
    let (prefix, value) = match word.split_at_checked(1) {
        Some((p @ ("@" | "#"), v)) => (p, v),
        _ => match word.split_once(':') {
            Some((p @ ("s" | "l" | "e" | "sp"), v)) => (p, v),
            _ => return false,
        },
    };
    if value.is_empty() {
        return false;
    }
    let value = value.to_string();
    match prefix {
        "@" if matches!(value.to_lowercase().as_str(), "me" | "moi") => {
            filter.assignees.push(UserSel::Me)
        }
        "@" if matches!(value.to_lowercase().as_str(), "none" | "nobody" | "unassigned") => {
            filter.assignees.push(UserSel::Nobody)
        }
        "@" => filter.assignees.push(UserSel::Named(value)),
        "#" => filter.types.push(value),
        "s" => filter.statuses.push(value),
        "l" => filter.labels.push(value),
        "e" => filter.epic = Some(value),
        _ => filter.sprint = Some(value),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(input: &str) -> Filter {
        match parse_query(input) {
            Query::Filter(f) => f,
            Query::Native(n) => panic!("unexpected native query {n}"),
        }
    }

    #[test]
    fn prefixes_become_fields_and_the_rest_is_text() {
        let f = filter("login @me #bug s:review l:backend e:PROJ-10 sp:14 timeout");
        assert_eq!(f.text.as_deref(), Some("login timeout"));
        assert_eq!(f.assignees, [UserSel::Me]);
        assert_eq!(f.types, ["bug"]);
        assert_eq!(f.statuses, ["review"]);
        assert_eq!(f.labels, ["backend"]);
        assert_eq!(f.epic.as_deref(), Some("PROJ-10"));
        assert_eq!(f.sprint.as_deref(), Some("14"));
    }

    #[test]
    fn a_name_after_at_is_an_assignee_and_moi_is_me() {
        assert_eq!(filter("@colin").assignees, [UserSel::Named("colin".into())]);
        assert_eq!(filter("@Moi").assignees, [UserSel::Me]);
        assert_eq!(
            filter("@me @AnaSilva @none").assignees,
            [UserSel::Me, UserSel::Named("AnaSilva".into()), UserSel::Nobody]
        );
        assert_eq!(filter("@unassigned").assignees, [UserSel::Nobody]);
    }

    #[test]
    fn repeated_prefixes_accumulate() {
        assert_eq!(filter("#bug #story").types, ["bug", "story"]);
    }

    #[test]
    fn a_bare_prefix_or_an_unknown_one_is_text() {
        assert_eq!(filter("# s: x:y").text.as_deref(), Some("# s: x:y"));
        assert_eq!(filter("").text, None);
    }

    #[test]
    fn a_leading_bang_is_a_native_query() {
        assert_eq!(parse_query("  !project = ACME "), Query::Native("project = ACME".into()));
    }
}
