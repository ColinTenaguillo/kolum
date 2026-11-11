//! Vim's jump list, for issues: where each issue was left, to go back to
//! it with `<C-o>` and forward again with `<C-i>`.

use super::detail::Tab;
use kolum_domain::IssueKey;
use std::collections::VecDeque;

/// Where an issue was left: how far down, on which activity tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub key: IssueKey,
    pub scroll: u16,
    pub tab: Tab,
}

impl Spot {
    /// An issue opened afresh: at its top, on its comments.
    pub fn top(key: IssueKey) -> Self {
        Spot { key, scroll: 0, tab: Tab::Comments }
    }
}

/// How many steps each way are kept; the oldest goes past that.
const CAP: usize = 100;

/// For the session only.
#[derive(Debug, Default)]
pub struct Jumps {
    back: Stack,
    forward: Stack,
}

/// The newest last.
type Stack = VecDeque<Spot>;

impl Jumps {
    /// An issue closed, or left for another: the way forward is dropped.
    pub fn leave(&mut self, spot: Spot) {
        push(&mut self.back, spot);
        self.forward.clear();
    }

    /// `n` steps back from `here`, the issue open if any, or as many as
    /// there are; those passed become the steps forward.
    pub fn back(&mut self, here: Option<Spot>, n: u32) -> Option<Spot> {
        walk(&mut self.back, &mut self.forward, here, n)
    }

    /// `n` steps forward again from `here`, which become steps back.
    pub fn forward(&mut self, here: Option<Spot>, n: u32) -> Option<Spot> {
        walk(&mut self.forward, &mut self.back, here, n)
    }
}

fn walk(from: &mut Stack, to: &mut Stack, here: Option<Spot>, n: u32) -> Option<Spot> {
    let mut at = step(from, to, here)?;
    for _ in 1..n {
        match step(from, to, Some(at.clone())) {
            Some(next) => at = next,
            None => break,
        }
    }
    Some(at)
}

/// The next spot of `from` that is not the issue open; `here` goes to `to`.
fn step(from: &mut Stack, to: &mut Stack, here: Option<Spot>) -> Option<Spot> {
    if let Some(here) = &here {
        while from.back().is_some_and(|s| s.key == here.key) {
            from.pop_back();
        }
    }
    let next = from.pop_back()?;
    if let Some(here) = here {
        push(to, here);
    }
    Some(next)
}

/// The same issue twice in a row is kept once, where it was last left.
fn push(stack: &mut Stack, spot: Spot) {
    match stack.back_mut() {
        Some(last) if last.key == spot.key => *last = spot,
        _ => {
            stack.push_back(spot);
            if stack.len() > CAP {
                stack.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(key: &str, scroll: u16) -> Spot {
        Spot { scroll, ..Spot::top(IssueKey::parse(key)) }
    }

    #[test]
    fn back_and_forward_walk_the_issues_left_in_order() {
        let mut jumps = Jumps::default();
        jumps.leave(at("A-1", 3));
        jumps.leave(at("A-2", 5));
        assert_eq!(jumps.back(None, 1), Some(at("A-2", 5)), "from the board, the last closed");
        assert_eq!(jumps.back(Some(at("A-2", 7)), 1), Some(at("A-1", 3)));
        assert_eq!(jumps.back(Some(at("A-1", 3)), 1), None, "nothing older");
        assert_eq!(jumps.forward(Some(at("A-1", 4)), 1), Some(at("A-2", 7)), "as it was left");
        assert_eq!(jumps.back(Some(at("A-2", 7)), 1), Some(at("A-1", 4)));
        jumps.leave(at("A-1", 4));
        assert_eq!(jumps.forward(None, 1), None, "leaving drops the way forward");
    }

    #[test]
    fn one_issue_twice_in_a_row_is_one_step() {
        let mut jumps = Jumps::default();
        jumps.leave(at("A-1", 1));
        jumps.leave(at("A-2", 1));
        jumps.leave(at("A-2", 9));
        assert_eq!(jumps.back(None, 1), Some(at("A-2", 9)));
        assert_eq!(jumps.back(None, 1), Some(at("A-1", 1)));
        jumps.leave(at("A-3", 0));
        assert_eq!(jumps.back(Some(at("A-3", 2)), 1), None, "the issue open is no step");
    }

    #[test]
    fn a_count_takes_as_many_steps_as_there_are() {
        let mut jumps = Jumps::default();
        for n in 1..=4 {
            jumps.leave(at(&format!("A-{n}"), 0));
        }
        assert_eq!(jumps.back(Some(at("A-5", 0)), 3), Some(at("A-2", 0)));
        assert_eq!(jumps.forward(Some(at("A-2", 0)), 9), Some(at("A-5", 0)), "the last there is");
        assert_eq!(jumps.back(None, 2), Some(at("A-3", 0)));
    }

    #[test]
    fn the_oldest_goes_past_a_hundred() {
        let mut jumps = Jumps::default();
        for n in 0..=CAP {
            jumps.leave(at(&format!("A-{n}"), 0));
        }
        let mut seen = 0;
        while let Some(spot) = jumps.back(None, 1) {
            seen += 1;
            assert_ne!(spot.key.as_str(), "A-0");
        }
        assert_eq!(seen, CAP);
    }
}
