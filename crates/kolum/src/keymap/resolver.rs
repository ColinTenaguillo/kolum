//! Turns key presses into actions: counts (`3j`), multi-key sequences
//! (`gg`), and the wait for more when a sequence could go on (`g` while
//! `gg` and `gd` both exist), which ends at the `timeoutlen` deadline.

use super::{Action, Binding, Key};
use crossterm::event::KeyCode;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fire {
        action: Action,
        count: Option<u32>,
    },
    /// A count or the start of a sequence: waiting for more.
    Pending,
    /// No binding starts with this key. In a text field the caller types it.
    Unhandled(Key),
    /// A sequence that went nowhere (`gx`); dropped, as vim does.
    Dropped,
}

/// The bindings in force, most specific first, and whether a text field has
/// the focus. In one, a key that types a character is never a binding.
pub struct Layers<'a> {
    pub tables: Vec<&'a [Binding]>,
    pub text: bool,
}

impl Layers<'_> {
    fn usable(&self, b: &Binding) -> bool {
        !self.text || b.keys[0].as_text().is_none()
    }

    fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.tables.iter().flat_map(|t| t.iter()).filter(|b| self.usable(b))
    }

    /// The action bound to exactly `seq` (the most specific layer wins),
    /// and whether some longer binding starts with it.
    fn lookup(&self, seq: &[Key]) -> (Option<Action>, bool) {
        let exact = self.bindings().find(|b| b.keys == seq).map(|b| b.action);
        let longer = self.bindings().any(|b| b.keys.len() > seq.len() && b.keys.starts_with(seq));
        (exact, longer)
    }
}

#[derive(Debug, Default)]
pub struct Resolver {
    pending: Vec<Key>,
    count: Option<u32>,
    deadline: Option<Instant>,
}

/// Vim caps counts too; past this a held key is a mistake, not a request.
const MAX_COUNT: u32 = 9999;

impl Resolver {
    pub fn feed(&mut self, key: Key, layers: &Layers, now: Instant, timeout: Duration) -> Outcome {
        if self.pending.is_empty() {
            if layers.text && key.as_text().is_some() {
                return Outcome::Unhandled(key);
            }
            if let Some(digit) = self.count_digit(key, layers.text) {
                let count = self.count.unwrap_or(0).saturating_mul(10).saturating_add(digit);
                self.count = Some(count.min(MAX_COUNT));
                return Outcome::Pending;
            }
        }
        self.pending.push(key);
        let (exact, longer) = layers.lookup(&self.pending);
        if longer {
            self.deadline = Some(now + timeout);
            return Outcome::Pending;
        }
        let was_sequence = self.pending.len() > 1;
        self.finish(exact).unwrap_or(if was_sequence {
            Outcome::Dropped
        } else {
            Outcome::Unhandled(key)
        })
    }

    /// `0` starts a count only after another digit: alone it is a motion.
    fn count_digit(&self, key: Key, text: bool) -> Option<u32> {
        match key.code {
            KeyCode::Char(c) if !text && key.mods.is_empty() => {
                c.to_digit(10).filter(|&d| d != 0 || self.count.is_some())
            }
            _ => None,
        }
    }

    fn finish(&mut self, action: Option<Action>) -> Option<Outcome> {
        self.pending.clear();
        self.deadline = None;
        let count = self.count.take();
        action.map(|action| Outcome::Fire { action, count })
    }

    /// When the wait for a longer sequence runs out, what was typed so far
    /// counts as it stands.
    pub fn on_timeout(&mut self, layers: &Layers, now: Instant) -> Option<Outcome> {
        if !self.deadline.is_some_and(|d| now >= d) {
            return None;
        }
        let (exact, _) = layers.lookup(&self.pending);
        Some(self.finish(exact).unwrap_or(Outcome::Dropped))
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// The count and keys typed so far, as the status line shows them.
    pub fn typed(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        count + &super::display_keys(&self.pending)
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.count.is_none()
    }

    /// What can follow the keys typed so far, for the which-key popup: the
    /// rest of each sequence and its action, the most specific layer first.
    pub fn continuations(&self, layers: &Layers) -> Vec<(String, Action)> {
        if self.pending.is_empty() {
            return vec![];
        }
        let mut out: Vec<(String, Action)> = vec![];
        for b in layers.bindings() {
            if b.keys.len() > self.pending.len() && b.keys.starts_with(&self.pending) {
                let rest = super::display_keys(&b.keys[self.pending.len()..]);
                if !out.iter().any(|(r, _)| *r == rest) {
                    out.push((rest, b.action));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::parse_keys;

    const T: Duration = Duration::from_millis(500);

    fn table(pairs: &[(&str, Action)]) -> Vec<Binding> {
        pairs
            .iter()
            .map(|(s, a)| Binding { keys: parse_keys(s, Key::char(' ')).unwrap(), action: *a })
            .collect()
    }

    fn feed_all(r: &mut Resolver, layers: &Layers, keys: &str, now: Instant) -> Vec<Outcome> {
        parse_keys(keys, Key::char(' '))
            .unwrap()
            .into_iter()
            .map(|k| r.feed(k, layers, now, T))
            .collect()
    }

    fn fire(action: Action, count: Option<u32>) -> Outcome {
        Outcome::Fire { action, count }
    }

    #[test]
    fn counts_prefix_motions_and_zero_alone_is_a_motion() {
        let board = table(&[("j", Action::Down), ("0", Action::FirstColumn)]);
        let layers = Layers { tables: vec![&board], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut r, &layers, "3j", now),
            [Outcome::Pending, fire(Action::Down, Some(3))]
        );
        assert_eq!(feed_all(&mut r, &layers, "0", now), [fire(Action::FirstColumn, None)]);
        assert_eq!(feed_all(&mut r, &layers, "10j", now).pop(), Some(fire(Action::Down, Some(10))));
        assert_eq!(feed_all(&mut r, &layers, "j", now), [fire(Action::Down, None)]);
    }

    #[test]
    fn a_sequence_waits_for_its_last_key() {
        let board = table(&[("gg", Action::Top), ("gd", Action::OpenDetail)]);
        let layers = Layers { tables: vec![&board], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut r, &layers, "g", now), [Outcome::Pending]);
        assert_eq!(r.typed(), "g");
        let mut next = r.continuations(&layers);
        next.sort();
        assert_eq!(next, [("d".into(), Action::OpenDetail), ("g".into(), Action::Top)]);
        assert_eq!(feed_all(&mut r, &layers, "d", now), [fire(Action::OpenDetail, None)]);
        assert!(r.is_idle());
        assert_eq!(feed_all(&mut r, &layers, "5gg", now).pop(), Some(fire(Action::Top, Some(5))));
    }

    #[test]
    fn a_sequence_that_goes_nowhere_is_dropped_with_its_count() {
        let board = table(&[("gg", Action::Top), ("j", Action::Down)]);
        let layers = Layers { tables: vec![&board], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut r, &layers, "2gx", now).pop(), Some(Outcome::Dropped));
        assert_eq!(feed_all(&mut r, &layers, "j", now), [fire(Action::Down, None)]);
        assert_eq!(feed_all(&mut r, &layers, "x", now), [Outcome::Unhandled(Key::char('x'))]);
    }

    #[test]
    fn a_prefix_that_is_also_bound_fires_at_the_timeout() {
        let board = table(&[("y", Action::CopyKey), ("yu", Action::CopyUrl)]);
        let layers = Layers { tables: vec![&board], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut r, &layers, "y", now), [Outcome::Pending]);
        assert_eq!(r.on_timeout(&layers, now + T / 2), None, "too early");
        assert_eq!(r.deadline(), Some(now + T));
        assert_eq!(r.on_timeout(&layers, now + T), Some(fire(Action::CopyKey, None)));
        assert!(r.is_idle());
        assert_eq!(r.on_timeout(&layers, now + T * 2), None, "nothing pending");
        // Typed in time, the longer one wins.
        assert_eq!(feed_all(&mut r, &layers, "yu", now).pop(), Some(fire(Action::CopyUrl, None)));
    }

    #[test]
    fn a_timeout_on_an_unbound_prefix_drops_it() {
        let board = table(&[("gg", Action::Top)]);
        let layers = Layers { tables: vec![&board], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        feed_all(&mut r, &layers, "g", now);
        assert_eq!(r.on_timeout(&layers, now + T), Some(Outcome::Dropped));
    }

    #[test]
    fn the_specific_layer_wins_over_global() {
        let board = table(&[("q", Action::Close)]);
        let global = table(&[("q", Action::Quit), ("?", Action::Help)]);
        let layers = Layers { tables: vec![&board, &global], text: false };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut r, &layers, "q?", now),
            [fire(Action::Close, None), fire(Action::Help, None)]
        );
    }

    #[test]
    fn in_a_text_field_characters_type_and_modified_keys_bind() {
        let picker = table(&[
            ("j", Action::Down),
            ("<C-n>", Action::Down),
            ("<leader>f", Action::PickerIssues),
        ]);
        let layers = Layers { tables: vec![&picker], text: true };
        let mut r = Resolver::default();
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut r, &layers, "j3", now),
            [Outcome::Unhandled(Key::char('j')), Outcome::Unhandled(Key::char('3'))]
        );
        assert_eq!(feed_all(&mut r, &layers, "<C-n>", now), [fire(Action::Down, None)]);
        // A leader of Space types a space in a text field.
        assert_eq!(feed_all(&mut r, &layers, " ", now), [Outcome::Unhandled(Key::char(' '))]);
        let bs = parse_keys("<BS>", Key::char(' ')).unwrap()[0];
        assert_eq!(r.feed(bs, &layers, now, T), Outcome::Unhandled(bs));
    }
}
