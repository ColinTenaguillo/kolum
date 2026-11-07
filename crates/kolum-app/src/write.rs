//! Changing issues: which transition moves a card to a column, and the
//! writes in flight, shown at once over the provider's board until it
//! answers.

use crate::board::BoardSnapshot;
use kolum_domain::{
    Board, EditableField, FieldValue, Issue, IssueKey, IssueUpdate, Rank, RichText, Status,
    StatusId, Transition, TransitionId, User, UserId,
};
use std::collections::BTreeMap;

/// What a card moved into `column` can take: the transitions whose target
/// status that column holds. Several is a choice for the user; none means
/// the workflow does not allow the move from here.
fn transitions_into(board: &Board, column: usize, transitions: &[Transition]) -> Vec<Transition> {
    let Some(column) = board.columns.get(column) else { return vec![] };
    transitions.iter().filter(|t| column.statuses.contains(&t.to.id)).cloned().collect()
}

/// What a move offers, once the provider listed the issue's transitions.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// No transition leads where the move goes.
    Nothing(Closed),
    /// The one way into the column asked for: taken without asking.
    Take(Transition),
    /// The user picks.
    Choose(Vec<Transition>),
}

/// Why a move has nothing to offer.
#[derive(Debug, Clone, PartialEq)]
pub enum Closed {
    /// No transition is open from where the issue is.
    Here,
    /// None leads into the column asked for, named when the board says.
    Into(Option<String>),
}

/// The transitions to offer an issue in status `here`: those to another
/// status (Jira's team-managed projects offer one to the status the issue
/// is already in: valid there, nothing to choose here); for a move into
/// `column`, only those into it when the board is known, a lone one taken
/// at once.
pub fn offer(
    transitions: Vec<Transition>,
    here: Option<&StatusId>,
    board: Option<&Board>,
    column: Option<usize>,
) -> Offer {
    let elsewhere: Vec<_> = transitions.into_iter().filter(|t| Some(&t.to.id) != here).collect();
    let mut offered = match (board, column) {
        (Some(board), Some(column)) => transitions_into(board, column, &elsewhere),
        _ => elsewhere,
    };
    match (column, offered.len()) {
        (Some(c), 0) => {
            let name = board.and_then(|b| b.columns.get(c)).map(|c| c.name.clone());
            Offer::Nothing(Closed::Into(name))
        }
        (None, 0) => Offer::Nothing(Closed::Here),
        (Some(_), 1) => Offer::Take(offered.remove(0)),
        _ => Offer::Choose(offered),
    }
}

/// A write as it is sent: the issue, how it shows at once, and what goes
/// to the provider.
pub type Outgoing = (IssueKey, Change, Write);

/// Why a transition moves none of a selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Unmoved {
    /// It asks for fields, which the web app has the form for: why, in words.
    AsksFor(String),
    /// Every issue is in its status already.
    AlreadyThere,
}

/// The writes that move a selection by `t`, picked among the transitions
/// of the first issue, each issue given with its status.
pub fn transition_writes(
    t: &Transition,
    statuses: &[(IssueKey, Option<StatusId>)],
) -> Result<Vec<Outgoing>, Unmoved> {
    if let Some(why) = asks_for(t) {
        return Err(Unmoved::AsksFor(why));
    }
    let first = statuses.first().and_then(|(_, status)| status.as_ref());
    let writes: Vec<_> = statuses
        .iter()
        .filter_map(|(key, here)| {
            let write = transition_write(t, first, here.as_ref())?;
            Some((key.clone(), Change::Status(t.to.clone()), write))
        })
        .collect();
    if writes.is_empty() {
        return Err(Unmoved::AlreadyThere);
    }
    Ok(writes)
}

/// The issues given to `user`, or to no one.
pub fn assign_writes(keys: &[IssueKey], user: Option<&User>) -> Vec<Outgoing> {
    let write = Write::Assign(user.map(|u| u.id.clone()));
    let change = Change::Assignee(user.cloned());
    keys.iter().map(|key| (key.clone(), change.clone(), write.clone())).collect()
}

/// A comment, in Markdown, on the issue: it changes nothing on the card.
pub fn comment_write(key: IssueKey, markdown: &str) -> Outgoing {
    let change = Change::Update(IssueUpdate::default());
    (key, change, Write::Comment(RichText::from_markdown(markdown)))
}

/// The issue next to `other` in the provider's order: after it when it
/// goes `down`, before it otherwise.
pub fn rank_write(key: IssueKey, other: IssueKey, down: bool) -> Outgoing {
    let rank = if down { Rank::After(other) } else { Rank::Before(other) };
    (key, Change::Rank(rank.clone()), Write::Rank(rank))
}

/// An edit of the issue, shown as it is sent.
pub fn update_write(key: IssueKey, update: IssueUpdate) -> Outgoing {
    (key, Change::Update(update.clone()), Write::Update(update))
}

/// One field set to `value`, or emptied; or why not: it is required.
pub fn field_update(
    field: &EditableField,
    value: Option<FieldValue>,
) -> Result<IssueUpdate, String> {
    if value.is_none() && field.required {
        return Err(format!("{} is required", field.name));
    }
    Ok(IssueUpdate::set(field, value))
}

/// What a choice field can be set to: its options, and nothing when it
/// may be emptied.
pub fn field_choices<T>(field: &EditableField, options: Vec<T>) -> Vec<Option<T>> {
    let mut choices: Vec<Option<T>> = options.into_iter().map(Some).collect();
    if !field.required {
        choices.push(None);
    }
    choices
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
fn apply(issue: &mut Issue, change: &Change) {
    match change {
        Change::Rank(_) => {}
        Change::Status(status) => issue.status = status.clone(),
        Change::Assignee(user) => issue.assignee = user.clone(),
        Change::Update(u) => u.apply_to(issue),
    }
}

/// Why a transition is not done here: it asks for fields, which the web
/// app has the form for.
fn asks_for(t: &Transition) -> Option<String> {
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
fn transition_write(
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

/// Takes the issue out of the list and puts it back next to the other;
/// nothing moves when either is not there.
fn move_issue(issues: &mut Vec<Issue>, key: &IssueKey, rank: &Rank) {
    let (Rank::Before(other) | Rank::After(other)) = rank;
    let place = |key: &IssueKey| issues.iter().position(|i| &i.key == key);
    let (Some(from), Some(to)) = (place(key), place(other)) else { return };
    if from == to {
        return;
    }
    let issue = issues.remove(from);
    // The other is one place up once the issue is out from above it.
    let at = to - usize::from(to > from);
    issues.insert(if matches!(rank, Rank::After(_)) { at + 1 } else { at }, issue);
}

/// What one change does to the board: a rank moves the card in the list,
/// any other change the card. Nothing when the card is not there.
fn change_board(snapshot: &mut BoardSnapshot, key: &IssueKey, change: &Change) {
    if let Change::Rank(rank) = change {
        move_issue(&mut snapshot.issues, key, rank);
    } else if let Some(issue) = snapshot.issues.iter_mut().find(|i| &i.key == key) {
        apply(issue, change);
    }
}

/// The provider's copy of the board once it read an issue back after a
/// write: the read in place of an older copy. An issue not on the board
/// stays off it: the next sync says if it joined.
pub fn written(snapshot: &mut BoardSnapshot, read: &Issue) {
    if let Some(issue) = snapshot.issues.iter_mut().find(|i| i.key == read.key)
        && supersedes(read, issue)
    {
        *issue = read.clone();
    }
}

/// Whether a copy of an issue that arrived replaces the one held. Replies
/// come back out of order: a copy read before a write, landing after the
/// one read back from it, must not undo it. The same `updated` is the same
/// version again.
pub fn supersedes(arrived: &Issue, held: &Issue) -> bool {
    arrived.updated >= held.updated
}

/// A copy of an issue, a board's most often, in place of the one shown,
/// keeping what the shown one read better: Jira sends rich text fields for
/// one issue and not for a board, and no description to a Jira Cloud
/// board. The description shown stays unless the copy carries one, even
/// emptied, and is newer, as a Data Center board's can be; or the one shown
/// carries none.
pub fn refresh(shown: &mut Issue, fresh: &Issue) {
    let only_here: Vec<_> = shown
        .fields
        .iter()
        .filter(|f| !fresh.fields.iter().any(|x| x.id == f.id))
        .cloned()
        .collect();
    let newer = fresh.description.is_some()
        && (shown.description.is_none() || fresh.updated > shown.updated);
    let description = std::mem::take(&mut shown.description);
    *shown = fresh.clone();
    shown.fields.extend(only_here);
    if !newer {
        shown.description = description;
    }
}

/// The issue whole: its last full read, brought up to the board's copy of
/// the same version (with the changes in flight on it); as read when the
/// board has none or an older one. `None` when the board's copy is newer
/// than the read, so that the description, which the copy may lack or
/// flatten, is read again first.
pub fn in_full(read: &Issue, board: Option<&Issue>) -> Option<Issue> {
    let mut whole = read.clone();
    let Some(board) = board else { return Some(whole) };
    if board.updated > read.updated {
        return None;
    }
    if supersedes(board, read) {
        refresh(&mut whole, board);
    }
    Some(whole)
}

/// A board snapshot as it lands over the one held: its cards and their
/// order, each card held in a newer copy keeping that copy.
pub fn land(held: Option<&BoardSnapshot>, mut arrived: BoardSnapshot) -> BoardSnapshot {
    let Some(held) = held else { return arrived };
    let kept: std::collections::HashMap<&IssueKey, &Issue> =
        held.issues.iter().map(|i| (&i.key, i)).collect();
    for issue in &mut arrived.issues {
        if let Some(kept) = kept.get(&issue.key).filter(|kept| !supersedes(issue, kept)) {
            *issue = (*kept).clone();
        }
    }
    arrived
}

/// The writes not yet seen in what the provider sends, as the user asked
/// for them: each shows over every snapshot until the provider refuses it,
/// or until a snapshot that holds it lands. A refusal only drops it;
/// nothing older is put back over newer news.
#[derive(Debug, Default)]
pub struct Pending {
    next: u64,
    changes: BTreeMap<u64, InFlight>,
}

#[derive(Debug)]
struct InFlight {
    key: IssueKey,
    change: Change,
    /// Taken by the provider when this was the last board asked for: a
    /// board asked for after holds it. `None` until the provider answers.
    taken_after: Option<u64>,
}

impl Pending {
    /// The id to send the write under.
    pub fn begin(&mut self, key: IssueKey, change: Change) -> u64 {
        self.next += 1;
        self.changes.insert(self.next, InFlight { key, change, taken_after: None });
        self.next
    }

    /// The provider refused it: it shows no more.
    pub fn refused(&mut self, op: u64) {
        self.changes.remove(&op);
    }

    /// The provider took it when `asked` was the last board asked for.
    /// The issue it read back after shows it, unless it is a rank (a read
    /// has no order); else it shows until a board asked for after lands.
    pub fn written(&mut self, op: u64, read_back: bool, asked: u64) {
        let Some(write) = self.changes.get_mut(&op) else { return };
        match read_back && !matches!(write.change, Change::Rank(_)) {
            true => self.refused(op),
            false => write.taken_after = Some(asked),
        }
    }

    /// The board asked for as `request` landed: it holds every write taken
    /// before it was asked for.
    pub fn landed(&mut self, request: u64) {
        self.changes.retain(|_, write| write.taken_after.is_none_or(|asked| asked >= request));
    }

    /// The board shown: the provider's, with each change in flight on top,
    /// in the order they were made.
    pub fn over(&self, provider: &BoardSnapshot) -> BoardSnapshot {
        let mut shown = provider.clone();
        for write in self.changes.values() {
            change_board(&mut shown, &write.key, &write.change);
        }
        shown
    }

    /// Whether the issue's description is on its way, not yet taken.
    pub fn saves_description(&self, key: &IssueKey) -> bool {
        self.changes.values().any(|w| {
            w.key == *key
                && w.taken_after.is_none()
                && matches!(&w.change, Change::Update(u) if u.description.is_some())
        })
    }

    /// One issue shown, the open one: the provider's copy with its own
    /// changes in flight on top.
    pub fn on(&self, mut issue: Issue) -> Issue {
        let key = issue.key.clone();
        for write in self.changes.values().filter(|w| w.key == key) {
            apply(&mut issue, &write.change);
        }
        issue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_board;
    use kolum_domain::{IssueReader, IssueWriter};
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

    fn names(ts: &[Transition]) -> Vec<&str> {
        ts.iter().map(|t| t.to.name.as_str()).collect()
    }

    /// A move into a column takes its lone way at once and asks among
    /// several; `t`, into no column, always asks; none says why.
    #[tokio::test]
    async fn a_move_offers_the_transitions_to_another_status() {
        let fake = FakeProvider::new();
        let board = load_board(&fake, None).await.unwrap().board;
        let key = IssueKey::parse("ACME-12"); // To Do
        let mut ts = fake.transitions(&key).await.unwrap();
        let todo = fake.issue(&key).await.unwrap().status;
        let mut to_itself = ts[0].clone();
        to_itself.to = todo.clone();
        ts.push(to_itself);
        let here = Some(&todo.id);
        let board = Some(&board);
        let Offer::Choose(all) = offer(ts.clone(), here, board, None) else { panic!() };
        assert_eq!(all.len(), ts.len() - 1, "not to where it is: {:?}", names(&all));
        let Offer::Take(review) = offer(ts.clone(), here, board, Some(3)) else { panic!() };
        assert_eq!(review.to.name, "In Review", "the lone way in, taken");
        let Offer::Choose(two) = offer(ts.clone(), here, board, Some(2)) else { panic!() };
        assert_eq!(names(&two), ["In Progress", "Blocked"]);
        let to_do = Offer::Nothing(Closed::Into(Some("To Do".into())));
        assert_eq!(offer(ts.clone(), here, board, Some(1)), to_do, "already there");
        let nowhere = Offer::Nothing(Closed::Into(None));
        assert_eq!(offer(ts.clone(), here, board, Some(9)), nowhere, "no such column");
        assert_eq!(offer(vec![], here, board, None), Offer::Nothing(Closed::Here));
        let one = vec![review.clone()];
        assert_eq!(offer(one.clone(), here, board, None), Offer::Choose(one.clone()), "`t` asks");
        // A column with no board to say what it holds: every way counts.
        assert_eq!(offer(one.clone(), here, None, Some(3)), Offer::Take(review), "lone, taken");
        assert_eq!(offer(vec![], here, None, Some(3)), nowhere);
    }

    /// The first issue's transition, each other issue by its own, none for
    /// one there already; not at all when the transition asks for fields.
    #[test]
    fn a_selection_moves_unless_the_transition_asks_for_fields() {
        use kolum_domain::{FieldId, StatusCategory};
        let status = |id: &str| Status {
            id: StatusId::new(id),
            name: id.into(),
            category: StatusCategory::Doing,
        };
        let mut t = Transition {
            id: TransitionId::new("51"),
            name: "Done".into(),
            to: status("done"),
            required_fields: vec![],
        };
        let key = |k: &str| IssueKey::parse(k);
        let statuses = [
            (key("A-1"), Some(status("todo").id)),
            (key("A-2"), Some(status("doing").id)),
            (key("A-3"), Some(status("done").id)),
            (key("A-4"), Some(status("todo").id)),
        ];
        let writes = transition_writes(&t, &statuses).unwrap();
        let by = |w: &Outgoing| (w.0.to_string(), w.2.clone());
        let by_its_own = Write::TransitionTo { id: t.id.clone(), to: t.to.clone() };
        assert_eq!(
            writes.iter().map(by).collect::<Vec<_>>(),
            [
                ("A-1".into(), Write::Transition(t.id.clone())),
                ("A-2".into(), by_its_own),
                ("A-4".into(), Write::Transition(t.id.clone())),
            ]
        );
        assert!(writes.iter().all(|w| w.1 == Change::Status(t.to.clone())), "shown at once");
        let there = transition_writes(&t, &statuses[2..3]);
        assert_eq!(there, Err(Unmoved::AlreadyThere), "nothing to send");
        t.required_fields = vec![FieldId::new("resolution")];
        let Err(Unmoved::AsksFor(why)) = transition_writes(&t, &statuses) else { panic!() };
        assert!(why.contains("`Done` asks for resolution"), "{why}");
    }

    /// Each write shows at once as what it changes on the card.
    #[test]
    fn each_action_is_a_write_and_the_change_it_shows() {
        let (one, two) = (IssueKey::parse("A-1"), IssueKey::parse("A-2"));
        let ana = User { id: UserId::new("u-ana"), display_name: "Ana".into(), avatar_url: None };
        let to_ana = assign_writes(&[one.clone(), two.clone()], Some(&ana));
        let ana_on = |key: &IssueKey| {
            (key.clone(), Change::Assignee(Some(ana.clone())), Write::Assign(Some(ana.id.clone())))
        };
        assert_eq!(to_ana, [ana_on(&one), ana_on(&two)]);
        let nobody = (one.clone(), Change::Assignee(None), Write::Assign(None));
        assert_eq!(assign_writes(std::slice::from_ref(&one), None), [nobody]);

        let (key, change, write) = comment_write(one.clone(), "*hi*");
        assert_eq!(key, one);
        assert_eq!(change, Change::Update(IssueUpdate::default()), "a comment changes no field");
        assert_eq!(write, Write::Comment(RichText::from_markdown("*hi*")));

        let after = Rank::After(two.clone());
        assert_eq!(rank_write(one.clone(), two.clone(), true).2, Write::Rank(after.clone()));
        assert_eq!(rank_write(one.clone(), two.clone(), true).1, Change::Rank(after));
        let before = Write::Rank(Rank::Before(two.clone()));
        assert_eq!(rank_write(one.clone(), two.clone(), false).2, before);

        let update = IssueUpdate { title: Some("New".into()), ..Default::default() };
        let sent = update_write(one.clone(), update.clone());
        assert_eq!(sent, (one, Change::Update(update.clone()), Write::Update(update)));
    }

    fn field(required: bool) -> EditableField {
        EditableField {
            id: kolum_domain::FieldId::new("customfield_1"),
            provider_id: kolum_domain::FieldId::new("customfield_1"),
            name: "Team".into(),
            kind: kolum_domain::FieldKind::Text,
            required,
            on_issue: true,
        }
    }

    #[test]
    fn a_required_field_cannot_be_emptied() {
        let set = Some(FieldValue::Text("Core".into()));
        assert_eq!(field_update(&field(true), None), Err("Team is required".into()));
        assert_eq!(field_update(&field(false), None), Ok(IssueUpdate::set(&field(false), None)));
        let update = IssueUpdate::set(&field(true), set.clone());
        assert_eq!(field_update(&field(true), set), Ok(update));
        let options = vec!["a".to_string(), "b".to_string()];
        let some = |o: &[&str]| o.iter().map(|o| Some(o.to_string())).collect::<Vec<_>>();
        assert_eq!(field_choices(&field(true), options.clone()), some(&["a", "b"]));
        let mut emptied = some(&["a", "b"]);
        emptied.push(None);
        assert_eq!(field_choices(&field(false), options), emptied, "none to empty it");
    }

    fn done() -> Status {
        Status {
            id: kolum_domain::StatusId::new("done"),
            name: "Done".into(),
            category: kolum_domain::StatusCategory::Done,
        }
    }

    fn synced_status(s: &BoardSnapshot) -> Status {
        issue(s, "ACME-12").status.clone()
    }

    fn issue<'a>(s: &'a BoardSnapshot, key: &str) -> &'a Issue {
        s.issues.iter().find(|i| i.key.as_str() == key).unwrap()
    }

    /// A change shows over each snapshot the provider sends until it is
    /// answered; then the snapshot alone, whatever it holds by then.
    #[tokio::test]
    async fn a_change_in_flight_shows_over_the_latest_snapshot() {
        let provider = load_board(&FakeProvider::new(), None).await.unwrap();
        let key = IssueKey::parse("ACME-12");
        let mut pending = Pending::default();
        let op = pending.begin(key.clone(), Change::Status(done()));
        let second = pending.begin(key.clone(), Change::Assignee(None));
        assert_ne!(op, second);
        let shown = pending.over(&provider);
        assert_eq!(issue(&shown, "ACME-12").status, done());
        assert_eq!(issue(&shown, "ACME-12").assignee, None);

        let mut synced = provider.clone();
        synced.issues.iter_mut().find(|i| i.key == key).unwrap().title = "Renamed".into();
        let shown = pending.over(&synced);
        assert_eq!(issue(&shown, "ACME-12").status, done(), "still on top");
        assert_eq!(issue(&shown, "ACME-12").title, "Renamed", "the sync's change kept");

        pending.refused(op);
        pending.refused(op);
        assert_eq!(issue(&pending.over(&synced), "ACME-12").status, synced_status(&synced));
        pending.refused(second);
        assert_eq!(pending.over(&synced), synced, "the provider's alone");
    }

    #[tokio::test]
    async fn a_rank_in_flight_moves_the_card_in_the_latest_order() {
        let provider = load_board(&FakeProvider::new(), None).await.unwrap();
        let order = |s: &BoardSnapshot| {
            s.issues.iter().take(3).map(|i| i.key.to_string()).collect::<Vec<_>>()
        };
        let mut pending = Pending::default();
        let after = Rank::After(IssueKey::parse("ACME-7"));
        let op = pending.begin(IssueKey::parse("ACME-5"), Change::Rank(after));
        assert_eq!(order(&pending.over(&provider)), ["ACME-6", "ACME-7", "ACME-5"]);
        let mut synced = provider.clone();
        synced.issues.swap(1, 2);
        assert_eq!(order(&pending.over(&synced)), ["ACME-7", "ACME-5", "ACME-6"]);
        pending.refused(op);
        assert_eq!(order(&pending.over(&synced)), ["ACME-5", "ACME-7", "ACME-6"]);
        let mut nothing = provider.clone();
        let five = IssueKey::parse("ACME-5");
        move_issue(&mut nothing.issues, &five, &Rank::After(five.clone()));
        assert_eq!(nothing, provider, "next to itself");
        let seven = IssueKey::parse("ACME-7"); // not first: no move is not a move to the top
        move_issue(&mut nothing.issues, &seven, &Rank::Before(IssueKey::parse("NOPE")));
        move_issue(&mut nothing.issues, &IssueKey::parse("NOPE"), &Rank::Before(five.clone()));
        assert_eq!(nothing, provider, "either not there");
        move_issue(&mut nothing.issues, &seven, &Rank::Before(five.clone()));
        assert_eq!(order(&nothing), ["ACME-7", "ACME-5", "ACME-6"], "up, before the other");
    }

    /// An issue off the board is not added to it; its own copy, the open
    /// one, takes its changes alone.
    #[tokio::test]
    async fn a_change_to_an_issue_off_the_board_shows_on_that_issue_only() {
        let provider = load_board(&FakeProvider::new(), None).await.unwrap();
        let mut pending = Pending::default();
        pending.begin(IssueKey::parse("NOPE-1"), Change::Status(done()));
        pending.begin(IssueKey::parse("ACME-12"), Change::Assignee(None));
        pending
            .begin(IssueKey::parse("NOPE-1"), Change::Rank(Rank::After(IssueKey::parse("ACME-5"))));
        let shown = pending.over(&provider);
        assert_eq!(shown.issues.len(), provider.issues.len());
        let mut off = provider.issues[0].clone();
        off.key = IssueKey::parse("NOPE-1");
        let assignee = off.assignee.clone();
        let off = pending.on(off);
        assert_eq!((off.status, off.assignee), (done(), assignee), "its own changes alone");
    }

    #[tokio::test]
    async fn an_update_sets_rich_custom_fields_by_id() {
        use kolum_domain::{FieldEdit, FieldId, FieldValue};
        let snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
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
        let mut pending = Pending::default();
        pending.begin(key.clone(), Change::Update(update));
        let snapshot = pending.over(&snapshot);
        let issue = snapshot.issues.iter().find(|i| i.key == key).unwrap();
        let values: Vec<(&str, &FieldValue)> =
            issue.fields.iter().map(|f| (f.id.as_str(), &f.value)).collect();
        assert_eq!(values, [("customfield_10030", &FieldValue::RichText(text))], "Team emptied");
    }

    /// The issue read back after a write replaces the board's copy, unless
    /// that is newer; an issue off the board stays off.
    #[tokio::test]
    async fn a_read_back_replaces_an_older_copy_on_the_board() {
        let mut snapshot = load_board(&FakeProvider::new(), None).await.unwrap();
        let issue_of = |s: &BoardSnapshot| issue(s, "ACME-9").clone();
        let mut read = issue_of(&snapshot);
        read.title = "As the provider has it".into();
        written(&mut snapshot, &read);
        assert_eq!(issue_of(&snapshot), read);
        let mut older = read.clone();
        older.title = "Older".into();
        older.updated -= time::Duration::minutes(1);
        written(&mut snapshot, &older);
        assert_eq!(issue_of(&snapshot), read, "an older read loses");
        let before = snapshot.clone();
        read.key = IssueKey::parse("NOPE-1");
        written(&mut snapshot, &read);
        assert_eq!(snapshot, before, "off the board, it stays off");
    }

    /// Taken by the provider, a change shows on until a board asked for
    /// after lands: no board asked for before can be trusted to hold it.
    /// Unless the issue read back after shows it: a read has no order, so
    /// a rank waits for the board, as does a write not read back.
    #[tokio::test]
    async fn a_taken_change_shows_until_a_board_asked_for_after_lands() {
        let provider = load_board(&FakeProvider::new(), None).await.unwrap();
        let mut pending = Pending::default();
        let read = pending.begin(IssueKey::parse("ACME-12"), Change::Status(done()));
        let unread = pending.begin(IssueKey::parse("ACME-9"), Change::Status(done()));
        let rank = Rank::After(IssueKey::parse("ACME-7"));
        let ranked = pending.begin(IssueKey::parse("ACME-5"), Change::Rank(rank));
        pending.written(read, true, 3);
        pending.written(unread, false, 3);
        pending.written(ranked, true, 3);
        pending.written(99, true, 3);
        let shown = pending.over(&provider);
        assert_eq!(issue(&shown, "ACME-12"), issue(&provider, "ACME-12"), "its read shows it");
        assert_eq!(issue(&shown, "ACME-9").status, done(), "not read back");
        assert_eq!(shown.issues[2].key.as_str(), "ACME-5", "a read has no order");
        pending.landed(3);
        assert_ne!(pending.over(&provider), provider, "asked for before it was taken");
        pending.landed(4);
        assert_eq!(pending.over(&provider), provider, "settled");
    }

    /// A description on its way: `e` would open the one before.
    #[test]
    fn a_description_on_its_way_is_known() {
        let key = IssueKey::parse("ACME-9");
        let mut pending = Pending::default();
        let text = |title: Option<&str>, description: Option<&str>| {
            Change::Update(IssueUpdate {
                title: title.map(String::from),
                description: description.map(RichText::from_markdown),
                ..Default::default()
            })
        };
        pending.begin(key.clone(), Change::Assignee(None));
        pending.begin(key.clone(), text(None, None));
        pending.begin(IssueKey::parse("ACME-5"), text(Some("T"), None));
        pending.begin(key.clone(), text(Some("T"), None));
        pending.begin(IssueKey::parse("ACME-5"), text(None, Some("d")));
        assert!(!pending.saves_description(&key), "not its description");
        let op = pending.begin(key.clone(), text(None, Some("d")));
        assert!(pending.saves_description(&key));
        pending.written(op, false, 1);
        assert!(!pending.saves_description(&key), "taken");
    }

    /// A board's copy over the issue read whole: its description only when
    /// it carries one, emptied or not, and is newer, or the read has none;
    /// the fields only the read has stay. Not newer than the read, the
    /// copy brings it up to date; newer, the read is not whole any more.
    #[tokio::test]
    async fn a_copy_brings_its_description_only_when_it_carries_one() {
        use kolum_domain::IssueReader;
        let read = FakeProvider::new().issue(&IssueKey::parse("ACME-10")).await.unwrap();
        let copy = |minutes, description: Option<&str>| {
            let mut copy = read.clone();
            // A board's copy has some fields, not the rich text ones.
            (copy.fields, copy.title) = (read.fields[..1].to_vec(), "Renamed".into());
            copy.updated += time::Duration::minutes(minutes);
            copy.description = description.map(RichText::from_markdown);
            copy
        };
        let refreshed = |shown: &Issue, copy: &Issue| {
            let mut shown = shown.clone();
            refresh(&mut shown, copy);
            shown
        };
        let cloud = refreshed(&read, &copy(1, None));
        assert_eq!((cloud.title.as_str(), &cloud.description), ("Renamed", &read.description));
        let mut fields = cloud.fields.clone();
        fields.sort_by_key(|f| f.id.clone());
        let mut wanted = read.fields.clone();
        wanted.sort_by_key(|f| f.id.clone());
        assert_eq!(fields, wanted, "the copy's fields, and the read's it lacks, once each");
        let emptied = Some(RichText::default());
        assert_eq!(refreshed(&read, &copy(1, Some(""))).description, emptied, "Data Center's");
        let same = refreshed(&read, &copy(0, Some("Flattened")));
        assert_eq!(same.description, read.description, "the same version: the read's");
        let none = Issue { description: None, ..read.clone() };
        let older = copy(-1, Some("Older"));
        assert_eq!(refreshed(&none, &older).description, older.description, "better than none");

        assert_eq!(in_full(&read, None), Some(read.clone()), "off the board");
        assert_eq!(in_full(&read, Some(&copy(0, None))), Some(refreshed(&read, &copy(0, None))));
        assert_eq!(in_full(&read, Some(&copy(1, None))), None, "read again first");
        let older = copy(-1, None);
        assert_eq!(in_full(&read, Some(&older)), Some(read.clone()), "an older copy stays under");
    }

    /// Replies come back out of order: a copy older than the one held does
    /// not replace it; the same or newer does. The snapshot that lands says
    /// which cards are on the board, and their order.
    #[tokio::test]
    async fn an_older_copy_does_not_replace_a_newer_one() {
        let held = load_board(&FakeProvider::new(), None).await.unwrap();
        let mut newer = held.clone();
        newer.issues[0].title = "Newer".into();
        newer.issues[0].updated += time::Duration::minutes(1);
        assert!(supersedes(&newer.issues[0], &held.issues[0]));
        assert!(supersedes(&held.issues[0], &held.issues[0]), "the same version again");
        assert!(!supersedes(&held.issues[0], &newer.issues[0]));

        let mut stale = held.clone();
        stale.issues.swap(0, 1);
        stale.issues.pop();
        let landed = land(Some(&newer), stale.clone());
        let keys = |s: &BoardSnapshot| s.issues.iter().map(|i| i.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&landed), keys(&stale), "the order and cards that landed");
        assert_eq!(landed.issues[1], newer.issues[0], "the newer copy kept");
        assert_eq!(landed.issues[0], stale.issues[0]);
        assert_eq!(land(Some(&held), newer.clone()), newer, "the newer copy lands");
        assert_eq!(land(None, stale.clone()), stale);
    }
}
