//! An issue's title and description as text to edit: `# Title`, then
//! Markdown. What an edit saves is only what moved since the text it began
//! from, and only over a description that has not moved meanwhile.

use kolum_domain::{Issue, IssueType, IssueUpdate, NewIssue, RichText};

/// The title and the description as they are edited: `# Title`, then
/// Markdown.
pub fn titled(issue: &Issue) -> String {
    format!("# {}\n\n{}", issue.title, issue.description_or_empty().to_markdown())
}

/// `# Title` on the first line, if there is one, and the rest.
fn split_title(text: &str) -> (Option<&str>, &str) {
    let text = text.trim_start();
    match text.split_once('\n') {
        Some((first, rest)) if first.trim() == "#" || first.starts_with("# ") => {
            (Some(first.trim_start_matches('#').trim()), rest.trim())
        }
        None if text.starts_with("# ") || text.trim() == "#" => {
            (Some(text.trim_start_matches('#').trim()), "")
        }
        _ => (None, text.trim()),
    }
}

/// `# Title` then Markdown, against the same it was edited from: only what
/// moved is sent. The description is compared as Markdown, so a rich text
/// that does not survive the round trip exactly is not rewritten for nothing.
pub fn description_update(from: &str, text: &str) -> IssueUpdate {
    let (was_title, was) = split_title(from);
    let (title, body) = split_title(text);
    IssueUpdate {
        title: title.filter(|t| !t.is_empty() && Some(*t) != was_title).map(String::from),
        description: (body != was).then(|| RichText::from_markdown(body)),
        ..Default::default()
    }
}

/// Whether the issue's description moved since the editing began `from`:
/// what was typed then goes back to the editor rather than over it.
pub fn changed_since(issue: &Issue, from: &str) -> bool {
    issue.description_or_empty().to_markdown().trim() != split_title(from).1
}

/// Two drafts of one issue as one: the title, and the description, each
/// from the later draft when it changed it, else from the one kept; with
/// the text it was changed from, so a save sends only what moved.
pub fn merge_drafts(kept: &(String, String), later: &(String, String)) -> (String, String) {
    let (kept_from, kept_text) = (split_title(&kept.0), split_title(&kept.1));
    let (later_from, later_text) = (split_title(&later.0), split_title(&later.1));
    let (from_title, title) = match later_text.0 != later_from.0 {
        true => (later_from.0, later_text.0),
        false => (kept_from.0, kept_text.0),
    };
    let (from_body, body) = match later_text.1 != later_from.1 {
        true => (later_from.1, later_text.1),
        false => (kept_from.1, kept_text.1),
    };
    let join =
        |title: Option<&str>, body: &str| format!("# {}\n\n{body}", title.unwrap_or_default());
    (join(from_title, from_body), join(title, body))
}

/// A new issue from its text, `# Title` then Markdown; none without a
/// title, which cancels.
pub fn new_issue(project: String, kind: IssueType, text: &str) -> Option<NewIssue> {
    let (title, body) = split_title(text);
    let title = title.filter(|t| !t.is_empty())?;
    let description = RichText::from_markdown(body);
    Some(NewIssue { project, kind, title: title.to_string(), description, parent: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolum_domain::IssueKind;

    #[test]
    fn the_title_is_a_first_line_that_starts_with_a_hash_and_a_space() {
        assert_eq!(split_title("  # Fix it \n\n body \n"), (Some("Fix it"), "body"));
        assert_eq!(split_title("#\nbody"), (Some(""), "body"), "a title emptied");
        assert_eq!(split_title("# Alone"), (Some("Alone"), ""));
        assert_eq!(split_title("#"), (Some(""), ""));
        assert_eq!(split_title("#tag first\nbody"), (None, "#tag first\nbody"), "a hashtag");
        assert_eq!(split_title("#tag"), (None, "#tag"));
        assert_eq!(split_title("just text"), (None, "just text"));
    }

    #[test]
    fn only_what_moved_is_sent() {
        let from = "# Fix it\n\nThe body";
        assert_eq!(description_update(from, from), IssueUpdate::default(), "no change");
        let renamed = description_update(from, "# Fixed\n\nThe body");
        assert_eq!(renamed.title.as_deref(), Some("Fixed"));
        assert_eq!(renamed.description, None, "the body stays");
        let rewritten = description_update(from, "# Fix it\n\nAnother body");
        assert_eq!(rewritten.title, None, "the title stays");
        assert_eq!(rewritten.description, Some(RichText::from_markdown("Another body")));
        assert_eq!(description_update(from, "#\n\nThe body").title, None, "no empty title");
    }

    #[tokio::test]
    async fn a_description_changed_since_the_editing_began_is_known() {
        let fake = kolum_provider_fake::FakeProvider::new();
        let mut issue = crate::load_board(&fake, None).await.unwrap().issues.remove(0);
        issue.description = Some(RichText::from_markdown("As it was"));
        let from = titled(&issue);
        assert!(!changed_since(&issue, &from));
        issue.title = "Renamed".into();
        assert!(!changed_since(&issue, &from), "the description alone counts");
        issue.description = Some(RichText::from_markdown("Moved"));
        assert!(changed_since(&issue, &from));
    }

    #[test]
    fn two_drafts_keep_what_each_changed() {
        let draft = |from: &str, text: &str| (from.to_string(), text.to_string());
        let kept = draft("# A\n\nbody", "# A\n\nmy body");
        let later = draft("# A\n\nbody", "# My A\n\nbody");
        assert_eq!(
            merge_drafts(&kept, &later),
            draft("# A\n\nbody", "# My A\n\nmy body"),
            "the later's title, the kept body"
        );
        let later = draft("# B\n\nnew", "# B\n\nnewer");
        assert_eq!(merge_drafts(&kept, &later), draft("# A\n\nnew", "# A\n\nnewer"));
    }

    #[test]
    fn a_new_issue_needs_a_title() {
        let task = IssueType { name: "Task".into(), kind: IssueKind::Task };
        let new = new_issue("ACME".into(), task.clone(), "# Do it\n\nlike *so*").unwrap();
        assert_eq!((new.project.as_str(), new.title.as_str()), ("ACME", "Do it"));
        assert_eq!(new.description, RichText::from_markdown("like *so*"));
        assert_eq!(new.parent, None);
        assert_eq!(new_issue("ACME".into(), task.clone(), "#\n\nbody"), None);
        assert_eq!(new_issue("ACME".into(), task, "no title"), None);
    }
}
