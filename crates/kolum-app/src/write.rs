//! Changing issues: which transition moves a card to a column, and the
//! optimistic ledger, where a change shows at once and is undone if the
//! provider refuses it.

use crate::board::BoardSnapshot;
use kolum_domain::{
    Board, Issue, IssueKey, IssueUpdate, Rank, RichText, Status, Transition, TransitionId, User,
    UserId,
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
        let ts = fake.transitions(&IssueKey::new("ACME-12")).await.unwrap(); // To Do
        let into =
            |c| transitions_into(&board, c, &ts).into_iter().map(|t| t.to.name).collect::<Vec<_>>();
        assert_eq!(into(2), ["In Progress", "Blocked"], "a column of two statuses: a choice");
        assert_eq!(into(3), ["In Review"]);
        assert!(into(1).is_empty(), "already there");
        assert!(into(9).is_empty(), "no such column");
    }

    #[tokio::test]
    async fn a_change_shows_at_once_and_is_undone_on_refusal() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let original = snapshot.clone();
        let key = IssueKey::new("ACME-12");
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
            &IssueKey::new("ACME-5"),
            &Change::Rank(Rank::After(IssueKey::new("ACME-7"))),
        );
        assert_eq!(order(&snapshot), ["ACME-6", "ACME-7", "ACME-5"]);
        assert!(ledger.rollback(&mut snapshot, op).is_some());
        assert_eq!(snapshot, original);
        assert!(
            !move_issue(
                &mut snapshot.issues,
                &IssueKey::new("ACME-5"),
                &Rank::After(IssueKey::new("ACME-5"))
            ),
            "next to itself"
        );
        assert!(!move_issue(
            &mut snapshot.issues,
            &IssueKey::new("ACME-5"),
            &Rank::Before(IssueKey::new("NOPE"))
        ));
        let nothing = ledger.begin(
            &mut snapshot,
            &IssueKey::new("NOPE"),
            &Change::Rank(Rank::Before(IssueKey::new("ACME-5"))),
        );
        assert_eq!(ledger.rollback(&mut snapshot, nothing), None, "nothing moved, nothing to undo");
        assert_eq!(snapshot, original);
    }

    #[tokio::test]
    async fn an_update_sets_rich_custom_fields_by_id() {
        use kolum_domain::{FieldId, FieldValue};
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let key = IssueKey::new("ACME-10");
        let text = RichText::from_markdown("- Reviewed by two people");
        let update = IssueUpdate {
            rich_fields: vec![(FieldId::new("customfield_10030"), text.clone())],
            ..Default::default()
        };
        Ledger::default().begin(&mut snapshot, &key, &Change::Update(update));
        let issue = snapshot.issues.iter().find(|i| i.key == key).unwrap();
        let values: Vec<(&str, &FieldValue)> =
            issue.fields.iter().map(|f| (f.id.as_str(), &f.value)).collect();
        assert_eq!(
            values,
            [
                ("customfield_10020", &FieldValue::Text("Platform".into())),
                ("customfield_10030", &FieldValue::RichText(text)),
            ]
        );
    }

    #[tokio::test]
    async fn a_settled_change_stays() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let key = IssueKey::new("ACME-9");
        let mut ledger = Ledger::default();
        let update = IssueUpdate {
            title: Some("T".into()),
            labels: Some(vec!["x".into()]),
            priority: Some(kolum_domain::Priority::Lowest),
            description: Some(RichText::from_markdown("d")),
            rich_fields: vec![],
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
            ledger.begin(&mut snapshot, &IssueKey::new("NOPE-1"), &Change::Assignee(None));
        assert_eq!(unknown, op + 1, "an issue off the board still gets an id");
        assert_eq!(ledger.pending(), 0);
    }
}
