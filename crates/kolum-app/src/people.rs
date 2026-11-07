//! Who is on a board: whom its issues can go to, and whose cards it holds.

use kolum_domain::{Issue, User};

/// Whom an issue can be given to: me, then whoever is on the board as an
/// assignee or a reporter, by name; then no one.
pub fn assignable(me: Option<&User>, issues: &[Issue]) -> Vec<Option<User>> {
    let mut users: Vec<User> = me.cloned().into_iter().collect();
    let mut others: Vec<User> = vec![];
    for issue in issues {
        for u in [&issue.assignee, &issue.reporter].into_iter().flatten() {
            if !users.iter().chain(&others).any(|o| o.id == u.id) {
                others.push(u.clone());
            }
        }
    }
    others.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    users.extend(others);
    users.into_iter().map(Some).chain([None]).collect()
}

/// Whose cards the board holds, and how many each: me first, even with
/// none, then the others by name, then no one when some cards have nobody.
pub fn assignees(me: Option<&User>, issues: &[Issue]) -> Vec<(Option<User>, usize)> {
    let mut counts: Vec<(Option<User>, usize)> = vec![];
    for issue in issues {
        let id = issue.assignee.as_ref().map(|a| &a.id);
        match counts.iter_mut().find(|(u, _)| u.as_ref().map(|u| &u.id) == id) {
            Some((_, n)) => *n += 1,
            None => counts.push((issue.assignee.clone(), 1)),
        }
    }
    let count_of = |id| counts.iter().find(|(u, _)| u.as_ref().map(|u| &u.id) == id);
    let mine = me.map(|me| (Some(me.clone()), count_of(Some(&me.id)).map_or(0, |(_, n)| *n)));
    let mut others: Vec<_> = counts
        .iter()
        .filter(|(u, _)| u.as_ref().is_some_and(|u| Some(&u.id) != me.map(|me| &me.id)))
        .cloned()
        .collect();
    others.sort_by(|(a, _), (b, _)| {
        a.as_ref().map(|u| &u.display_name).cmp(&b.as_ref().map(|u| &u.display_name))
    });
    let nobody = count_of(None).cloned();
    mine.into_iter().chain(others).chain(nobody).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_board;
    use kolum_domain::UserId;
    use kolum_provider_fake::FakeProvider;

    fn user(id: &str, name: &str) -> User {
        User { id: UserId::new(id), display_name: name.into(), avatar_url: None }
    }

    async fn board() -> Vec<Issue> {
        load_board(&FakeProvider::new(), None).await.unwrap().issues
    }

    fn named(users: &[Option<User>]) -> Vec<&str> {
        users.iter().map(|u| u.as_ref().map_or("nobody", |u| u.display_name.as_str())).collect()
    }

    #[tokio::test]
    async fn an_issue_goes_to_me_then_to_anyone_on_the_board_by_name_then_to_no_one() {
        let mut issues = board().await;
        let me = user("me", "Zoe Me");
        issues[0].reporter = Some(me.clone());
        issues[1].assignee = Some(user("z", "Aaron Late"));
        let choices = assignable(Some(&me), &issues);
        let names = named(&choices);
        assert_eq!(names[0], "Zoe Me", "me first, though last by name");
        assert_eq!(names[1], "Aaron Late", "then by name");
        assert_eq!(names.last(), Some(&"nobody"));
        assert_eq!(names.iter().filter(|n| **n == "Zoe Me").count(), 1, "each once");
        let mut sorted = names[1..names.len() - 1].to_vec();
        sorted.sort();
        assert_eq!(sorted, names[1..names.len() - 1], "{names:?}");
        assert!(names.len() > 3, "assignees and reporters both: {names:?}");
        assert_eq!(named(&assignable(None, &[])), ["nobody"]);
    }

    #[tokio::test]
    async fn the_cards_are_counted_by_assignee_me_first_and_nobody_last() {
        let mut issues = board().await;
        let me = user("me", "Zoe Me");
        issues[0].assignee = Some(me.clone());
        issues[1].assignee = Some(me.clone());
        issues[2].assignee = None;
        let people = assignees(Some(&me), &issues);
        let (first, mine) = &people[0];
        assert_eq!((first.as_ref(), *mine), (Some(&me), 2));
        assert_eq!(people.last().map(|(u, _)| u.is_none()), Some(true), "nobody last");
        let others: Vec<&str> = people[1..people.len() - 1]
            .iter()
            .filter_map(|(u, _)| u.as_ref())
            .map(|u| u.display_name.as_str())
            .collect();
        let mut sorted = others.clone();
        sorted.sort();
        assert_eq!(others, sorted, "the others by name");
        let total: usize = people.iter().map(|(_, n)| n).sum();
        assert_eq!(total, issues.len(), "every card counted once");

        let stranger = user("x", "Not Here");
        assert_eq!(
            assignees(Some(&stranger), &issues[..0]),
            [(Some(stranger), 0)],
            "me even with none"
        );
        issues.iter_mut().for_each(|i| i.assignee = Some(me.clone()));
        assert_eq!(assignees(None, &issues), [(Some(me), issues.len())], "no nobody without one");
    }
}
