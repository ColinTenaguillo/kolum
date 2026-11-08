//! One key press, and the vim notation for writing sequences of them:
//! `gg`, `<C-d>`, `<leader>f`, `<S-Tab>`, `<CR>`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    /// Shift is folded into the character: terminals disagree on whether
    /// `L` arrives with SHIFT set, and `L` is `L` either way.
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        match code {
            KeyCode::Char(c) => Key { code: KeyCode::Char(c), mods: mods - KeyModifiers::SHIFT },
            KeyCode::BackTab => Key { code: KeyCode::Tab, mods: mods | KeyModifiers::SHIFT },
            code => Key { code, mods },
        }
    }

    pub fn char(c: char) -> Self {
        Key::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// A key that types itself into a text field: a character with neither
    /// Control nor Alt.
    pub fn as_text(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if self.mods.is_empty() => Some(c),
            _ => None,
        }
    }
}

impl From<KeyEvent> for Key {
    fn from(e: KeyEvent) -> Self {
        Key::new(e.code, e.modifiers)
    }
}

const NAMES: &[(&str, KeyCode)] = &[
    ("CR", KeyCode::Enter),
    ("Enter", KeyCode::Enter),
    ("Return", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("BS", KeyCode::Backspace),
    ("Backspace", KeyCode::Backspace),
    ("Space", KeyCode::Char(' ')),
    ("lt", KeyCode::Char('<')),
    ("Bar", KeyCode::Char('|')),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("Del", KeyCode::Delete),
    ("Insert", KeyCode::Insert),
];

/// The inside of `<...>`: modifiers, then a name or a single character.
fn parse_special(token: &str) -> Option<Key> {
    let (mods, rest) = modifiers(token)?;
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) if mods.contains(KeyModifiers::SHIFT) => {
            KeyCode::Char(c.to_ascii_uppercase())
        }
        // `<C-D>` is `<C-d>`: terminals cannot send a distinct Ctrl-Shift-d.
        (Some(c), None) if mods.contains(KeyModifiers::CONTROL) => {
            KeyCode::Char(c.to_ascii_lowercase())
        }
        (Some(c), None) => KeyCode::Char(c),
        _ => named(rest)?,
    };
    Some(Key::new(code, mods))
}

/// The `C-`, `A-` (or `M-`) and `S-` in front, and what follows them.
fn modifiers(token: &str) -> Option<(KeyModifiers, &str)> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = token;
    while let Some((m, tail)) = rest.split_once('-').filter(|(m, t)| m.len() == 1 && !t.is_empty())
    {
        mods |= match m.to_ascii_uppercase().as_str() {
            "C" => KeyModifiers::CONTROL,
            "A" | "M" => KeyModifiers::ALT,
            "S" => KeyModifiers::SHIFT,
            _ => return None,
        };
        rest = tail;
    }
    Some((mods, rest))
}

/// `F1` to `F12`, or a name like `CR` or `Esc`.
fn named(name: &str) -> Option<KeyCode> {
    match name.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u8>().ok()) {
        Some(n) => Some(KeyCode::F(n)),
        None => NAMES.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, k)| *k),
    }
}

/// Reads a sequence in vim notation. `<leader>` stands for `leader`.
pub fn parse_keys(s: &str, leader: Key) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>').filter(|&e| e > 1)
        {
            let token = &rest[1..end];
            keys.push(if token.eq_ignore_ascii_case("leader") {
                leader
            } else {
                parse_special(token).ok_or_else(|| format!("`<{token}>` is not a key"))?
            });
            rest = &rest[end + 1..];
        } else {
            keys.push(Key::char(c));
            rest = &rest[c.len_utf8()..];
        }
    }
    if keys.is_empty() {
        return Err("an empty key sequence".into());
    }
    Ok(keys)
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char('<') => "lt".to_string(),
            KeyCode::Char(c) if self.mods.is_empty() => return write!(f, "{c}"),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("F{n}"),
            code => NAMES
                .iter()
                .find(|(_, k)| *k == code)
                .map(|(n, _)| n.to_string())
                .unwrap_or_else(|| format!("{code:?}")),
        };
        let mods =
            [(KeyModifiers::CONTROL, "C-"), (KeyModifiers::ALT, "A-"), (KeyModifiers::SHIFT, "S-")];
        let held: String =
            mods.iter().filter(|(m, _)| self.mods.contains(*m)).map(|(_, p)| *p).collect();
        write!(f, "<{held}{name}>")
    }
}

pub fn display_keys(keys: &[Key]) -> String {
    keys.iter().map(Key::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> Key {
        Key::char(' ')
    }

    fn parse(s: &str) -> Vec<Key> {
        parse_keys(s, space()).unwrap()
    }

    #[test]
    fn plain_characters_are_one_key_each() {
        assert_eq!(parse("gg"), vec![Key::char('g'), Key::char('g')]);
        assert_eq!(parse("$"), vec![Key::char('$')]);
        assert_eq!(parse("L"), vec![Key::char('L')]);
    }

    #[test]
    fn angle_brackets_name_special_keys_and_modifiers() {
        let ctrl_d = Key::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(parse("<C-d>"), vec![ctrl_d]);
        assert_eq!(parse("<c-D>"), vec![ctrl_d]);
        assert_eq!(parse("<CR>"), vec![Key::new(KeyCode::Enter, KeyModifiers::NONE)]);
        assert_eq!(parse("<enter>"), parse("<CR>"));
        assert_eq!(parse("<S-Tab>"), vec![Key::new(KeyCode::BackTab, KeyModifiers::NONE)]);
        assert_eq!(parse("<S-a>"), vec![Key::char('A')]);
        assert_eq!(parse("<A-x>"), parse("<M-x>"));
        assert_eq!(parse("<F5>"), vec![Key::new(KeyCode::F(5), KeyModifiers::NONE)]);
        assert_eq!(parse("<C-Space>"), vec![Key::new(KeyCode::Char(' '), KeyModifiers::CONTROL)]);
    }

    #[test]
    fn leader_expands_to_the_configured_key() {
        assert_eq!(parse("<leader>f"), vec![space(), Key::char('f')]);
        assert_eq!(parse_keys("<Leader>f", Key::char(',')).unwrap()[0], Key::char(','));
    }

    #[test]
    fn a_lone_angle_bracket_is_itself() {
        assert_eq!(parse("<"), vec![Key::char('<')]);
        assert_eq!(parse("<>"), vec![Key::char('<'), Key::char('>')]);
        assert_eq!(parse("<lt>"), vec![Key::char('<')]);
    }

    #[test]
    fn unknown_names_and_empty_sequences_are_errors() {
        assert_eq!(parse_keys("<Foo>", space()), Err("`<Foo>` is not a key".into()));
        assert_eq!(parse_keys("<X-a>", space()), Err("`<X-a>` is not a key".into()));
        assert!(parse_keys("", space()).is_err());
    }

    #[test]
    fn shift_from_the_terminal_is_folded_into_the_character() {
        let event = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        assert_eq!(Key::from(event), Key::char('L'));
    }

    #[test]
    fn display_writes_the_notation_back() {
        for s in ["gg", "<C-d>", "<CR>", "<Space>f", "<S-Tab>", "<A-x>", "<lt>", "<F5>"] {
            assert_eq!(display_keys(&parse(s)), s);
        }
    }
}
