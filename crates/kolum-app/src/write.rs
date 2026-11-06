//! Changing issues: which transition moves a card to a column, and the
//! optimistic ledger, where a change shows at once and is undone if the
//! provider refuses it.

use crate::board::BoardSnapshot;
use kolum_domain::{
    Board, Issue, IssueKey, IssueUpdate, Rank, RichText, Status, StatusId, Transition,
    TransitionId, User, UserId,
};
use std::collections::BTreeMap;

/// What a card moved into `column` can take: the transitions whose target
/// status that column holds. Several is a choice for the user; none means
/// the workflow does not allow the move from here.
pub fn transitions_into(
    board: &Board,
    column: usize,
    transitions: &[Transition],
) -> Vec<Transition> {
    let Some(column) = board.columns.get(column) else { return vec![] };
    transitions.iter().filter(|t| column.statuses.contains(&t.to.id)).cloned().collect()
}

/// A change sent to the provider.
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    Transition(TransitionId),
    /// By whichever transition takes the issue to this status from where it
    /// is: each issue of a selection may be in another status.
    TransitionTo {
        id: TransitionId,
        to: Status,
    },
    Assign(Option<UserId>),
    Comment(RichText),
    Update(IssueUpdate),
    Rank(Rank),
}

/// How a write shows on screen before the provider has answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Status(Status),
    Assignee(Option<User>),
    Update(IssueUpdate),
    /// A place in the board's order.
    Rank(Rank),
}

/// What a change does to one issue. A rank moves it in the board's list
/// instead: see [`move_issue`].
pub fn apply(issue: &mut Issue, change: &Change) {
    match change {
        Change::Rank(_) => {}
        Change::Status(status) => issue.status = status.clone(),
        Change::Assignee(user) => issue.assignee = user.clone(),
        Change::Update(u) => u.apply_to(issue),
    }
}

/// Why a transition is not done here: it asks for fields, which the web
/// app has the form for.
pub fn asks_for(t: &Transition) -> Option<String> {
    let fields: Vec<&str> = t.required_fields.iter().map(|f| f.as_str()).collect();
    (!fields.is_empty()).then(|| format!("`{}` asks for {}", t.name, fields.join(", ")))
}

/// Among an issue's transitions, `id`, the one picked, when it has it, else
/// the first to `status`: refused when there is none, or it asks for fields.
pub fn transition_to(
    transitions: &[Transition],
    id: &TransitionId,
    status: &Status,
) -> kolum_domain::Result<TransitionId> {
    let refused = |why: String| Err(kolum_domain::DomainError::Conflict(why));
    let to_status = |t: &&Transition| t.to.id == status.id;
    let picked = transitions.iter().filter(to_status).find(|t| t.id == *id);
    let Some(t) = picked.or_else(|| transitions.iter().find(to_status)) else {
        return refused(format!("no transition to {} from here", status.name));
    };
    match asks_for(t) {
        Some(why) => refused(why),
        None => Ok(t.id.clone()),
    }
}

/// How one issue of a selection moves by `t`, picked among the transitions
/// of the first, in status `first`: by it from the same status, by its own
/// from another; not at all when it is there already.
pub fn transition_write(
    t: &Transition,
    first: Option<&StatusId>,
    here: Option<&StatusId>,
) -> Option<Write> {
    if here == Some(&t.to.id) {
        return None;
    }
    Some(match here == first {
        true => Write::Transition(t.id.clone()),
        false => Write::TransitionTo { id: t.id.clone(), to: t.to.clone() },
    })
}

/// Takes the issue out of the list and puts it back next to the other.
/// False, and nothing moved, when either is not there.
pub fn move_issue(issues: &mut Vec<Issue>, key: &IssueKey, rank: &Rank) -> bool {
    let (Rank::Before(other) | Rank::After(other)) = rank;
    let Some(from) = issues.iter().position(|i| &i.key == key) else { return false };
    if key == other || !issues.iter().any(|i| &i.key == other) {
        return false;
    }
    let issue = issues.remove(from);
    let at = issues.iter().position(|i| &i.key == other).unwrap_or(0);
    issues.insert(if matches!(rank, Rank::After(_)) { at + 1 } else { at }, issue);
    true
}

/// What a write changed, to put it back.
#[derive(Debug)]
enum Before {
    Issue(Box<Issue>),
    Order(Vec<IssueKey>),
}

/// The writes in flight, each with the issue as it was before. When two
/// touch the same issue and the first fails, the issue goes back to before
/// both; the reply to the second then brings the provider's truth.
#[derive(Debug, Default)]
pub struct Ledger {
    next: u64,
    before: BTreeMap<u64, Before>,
}

impl Ledger {
    /// Applies the change on the board, and gives the id to send the write
    /// under. `None` when the issue is not on the board: nothing to show,
    /// but the write still goes out, under a fresh id.
    pub fn begin(&mut self, snapshot: &mut BoardSnapshot, key: &IssueKey, change: &Change) -> u64 {
        self.next += 1;
        if let Change::Rank(rank) = change {
            let order = snapshot.issues.iter().map(|i| i.key.clone()).collect();
            if move_issue(&mut snapshot.issues, key, rank) {
                self.before.insert(self.next, Before::Order(order));
            }
        } else if let Some(issue) = snapshot.issues.iter_mut().find(|i| &i.key == key) {
            self.before.insert(self.next, Before::Issue(Box::new(issue.clone())));
            apply(issue, change);
        }
        self.next
    }
    /// The provider took it: forget the old state.
    pub fn settle(&mut self, op: u64) {
        self.before.remove(&op);
    }

    /// The provider refused it: put the issue back as it was.
    pub fn rollback(&mut self, snapshot: &mut BoardSnapshot, op: u64) -> Option<IssueKey> {
        match self.before.remove(&op)? {
            Before::Issue(old) => {
                let key = old.key.clone();
                if let Some(issue) = snapshot.issues.iter_mut().find(|i| i.key == key) {
                    *issue = *old;
                }
                Some(key)
            }
            // The old order, every issue it knew back in its place; one that
            // arrived since stays at the end.
            Before::Order(order) => {
                let place =
                    |i: &Issue| order.iter().position(|k| *k == i.key).unwrap_or(usize::MAX);
                snapshot.issues.sort_by_key(place);
                order.first().cloned()
            }
        }
    }
    pub fn pending(&self) -> usize {
        self.before.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_board;
    use kolum_domain::IssueWriter;
    use kolum_provider_fake::FakeProvider;

    #[tokio::test]
    async fn the_transitions_into_a_column_are_those_to_its_statuses() {
        let fake = FakeProvider::new();
        let board = load_board(&fake, None).await.unwrap().board;
        let ts = fake.transitions(&IssueKey::parse("ACME-12")).await.unwrap(); // To Do
        let into =
            |c| transitions_into(&board, c, &ts).into_iter().map(|t| t.to.name).collect::<Vec<_>>();
        assert_eq!(into(2), ["In Progress", "Blocked"], "a column of two statuses: a choice");
        assert_eq!(into(3), ["In Review"]);
        assert!(into(1).is_empty(), "already there");
        assert!(into(9).is_empty(), "no such column");
    }

    /// Each issue of a selection goes by its own transition to the status
    /// picked: their statuses, and so their transitions, may differ.
    #[test]
    fn the_transition_to_a_status_is_the_issues_own() {
        use kolum_domain::{DomainError, FieldId, StatusCategory, StatusId};
        let status = |id: &str| Status {
            id: StatusId::new(id),
            name: format!("In {id}"),
            category: StatusCategory::Doing,
        };
        let t = |id: &str, to: &str, required: &[&str]| Transition {
            id: TransitionId::new(id),
            name: format!("Go {to}"),
            to: status(to),
            required_fields: required.iter().map(|f| FieldId::new(*f)).collect(),
        };
        let from_todo = [t("11", "review", &[]), t("21", "done", &[]), t("22", "done", &[])];
        let from_doing = [t("31", "review", &[]), t("41", "done", &["resolution"])];
        let to =
            |id: &str, to: &str| transition_to(&from_todo, &TransitionId::new(id), &status(to));
        assert_eq!(to("22", "done"), Ok(TransitionId::new("22")), "the one picked, Won't Do say");
        assert_eq!(to("31", "review"), Ok(TransitionId::new("11")), "else the first to there");
        let picked = TransitionId::new("11");
        let review = transition_to(&from_doing, &picked, &status("review"));
        assert_eq!(review, Ok(TransitionId::new("31")), "the issue's own");
        let Err(DomainError::Conflict(none)) = to("11", "qa") else { panic!() };
        assert!(none.contains("In qa"), "{none}");
        let Err(DomainError::Conflict(asks)) = transition_to(&from_doing, &picked, &status("done"))
        else {
            panic!()
        };
        assert!(asks.contains("resolution"), "{asks}");
    }

    /// One issue of a selection moved by the transition picked among the
    /// first's: by it when in the first's status, by its own when not, and
    /// not at all when there already.
    #[test]
    fn each_issue_of_a_selection_moves_by_what_its_status_allows() {
        use kolum_domain::{StatusCategory, StatusId};
        let status = |id: &str| Status {
            id: StatusId::new(id),
            name: id.into(),
            category: StatusCategory::Doing,
        };
        let t = Transition {
            id: TransitionId::new("51"),
            name: "Won't do".into(),
            to: status("done"),
            required_fields: vec![],
        };
        let (todo, doing, done) = (status("todo").id, status("doing").id, status("done").id);
        let write = |here: &StatusId| transition_write(&t, Some(&todo), Some(here));
        assert_eq!(write(&todo), Some(Write::Transition(t.id.clone())));
        let by_its_own = Write::TransitionTo { id: t.id.clone(), to: t.to.clone() };
        assert_eq!(write(&doing), Some(by_its_own));
        assert_eq!(write(&done), None, "there already");
    }

    #[tokio::test]
    async fn a_change_shows_at_once_and_is_undone_on_refusal() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let original = snapshot.clone();
        let key = IssueKey::parse("ACME-12");
        let done = Status {
            id: kolum_domain::StatusId::new("done"),
            name: "Done".into(),
            category: kolum_domain::StatusCategory::Done,
        };
        let mut ledger = Ledger::default();
        let op = ledger.begin(&mut snapshot, &key, &Change::Status(done.clone()));
        let issue = |s: &BoardSnapshot| s.issues.iter().find(|i| i.key == key).unwrap().clone();
        assert_eq!(issue(&snapshot).status, done);
        assert_eq!(ledger.pending(), 1);

        let second = ledger.begin(&mut snapshot, &key, &Change::Assignee(None));
        assert_eq!(ledger.rollback(&mut snapshot, op), Some(key.clone()));
        assert_eq!(snapshot, original, "back to before both");
        ledger.settle(second);
        assert_eq!(ledger.pending(), 0);
        assert_eq!(ledger.rollback(&mut snapshot, op), None, "undone once");
    }

    #[tokio::test]
    async fn a_rank_moves_the_card_at_once_and_a_refusal_puts_the_order_back() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let original = snapshot.clone();
        let order = |s: &BoardSnapshot| {
            s.issues.iter().take(3).map(|i| i.key.to_string()).collect::<Vec<_>>()
        };
        let mut ledger = Ledger::default();
        let op = ledger.begin(
            &mut snapshot,
            &IssueKey::parse("ACME-5"),
            &Change::Rank(Rank::After(IssueKey::parse("ACME-7"))),
        );
        assert_eq!(order(&snapshot), ["ACME-6", "ACME-7", "ACME-5"]);
        assert!(ledger.rollback(&mut snapshot, op).is_some());
        assert_eq!(snapshot, original);
        assert!(
            !move_issue(
                &mut snapshot.issues,
                &IssueKey::parse("ACME-5"),
                &Rank::After(IssueKey::parse("ACME-5"))
            ),
            "next to itself"
        );
        assert!(!move_issue(
            &mut snapshot.issues,
            &IssueKey::parse("ACME-5"),
            &Rank::Before(IssueKey::parse("NOPE"))
        ));
        let nothing = ledger.begin(
            &mut snapshot,
            &IssueKey::parse("NOPE"),
            &Change::Rank(Rank::Before(IssueKey::parse("ACME-5"))),
        );
        assert_eq!(ledger.rollback(&mut snapshot, nothing), None, "nothing moved, nothing to undo");
        assert_eq!(snapshot, original);
    }

    #[tokio::test]
    async fn an_update_sets_rich_custom_fields_by_id() {
        use kolum_domain::{FieldEdit, FieldId, FieldValue};
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let key = IssueKey::parse("ACME-10");
        let text = RichText::from_markdown("- Reviewed by two people");
        let edit = |id: &str, value| FieldEdit {
            id: FieldId::new(id),
            provider_id: FieldId::new(id),
            name: Some(id.into()),
            value,
        };
        let update = IssueUpdate {
            fields: vec![
                edit("customfield_10030", Some(FieldValue::RichText(text.clone()))),
                edit("customfield_10020", None),
            ],
            ..Default::default()
        };
        Ledger::default().begin(&mut snapshot, &key, &Change::Update(update));
        let issue = snapshot.issues.iter().find(|i| i.key == key).unwrap();
        let values: Vec<(&str, &FieldValue)> =
            issue.fields.iter().map(|f| (f.id.as_str(), &f.value)).collect();
        assert_eq!(values, [("customfield_10030", &FieldValue::RichText(text))], "Team emptied");
    }

    #[tokio::test]
    async fn a_settled_change_stays() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let key = IssueKey::parse("ACME-9");
        let mut ledger = Ledger::default();
        let update = IssueUpdate {
            title: Some("T".into()),
            labels: Some(vec!["x".into()]),
            priority: Some(kolum_domain::Priority::Lowest),
            description: Some(RichText::from_markdown("d")),
            fields: vec![],
        };
        let op = ledger.begin(&mut snapshot, &key, &Change::Update(update));
        ledger.settle(op);
        let issue = snapshot.issues.iter().find(|i| i.key == key).unwrap();
        assert_eq!(
            (issue.title.as_str(), issue.labels.as_slice(), issue.priority),
            ("T", &["x".to_string()][..], Some(kolum_domain::Priority::Lowest))
        );
        assert_eq!(issue.description.plain_text(), "d");
        assert_eq!(ledger.rollback(&mut snapshot, op), None);
        let unknown =
            ledger.begin(&mut snapshot, &IssueKey::parse("NOPE-1"), &Change::Assignee(None));
        assert_eq!(unknown, op + 1, "an issue off the board still gets an id");
        assert_eq!(ledger.pending(), 0);
    }
}
