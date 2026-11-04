//! The key table: `(context, key sequence) -> Action`, the defaults, and
//! the config that rebinds them.
//!
//! Which contexts are in force depends on what has the focus (see
//! `ui::App::layers`); a binding in a specific context wins over `global`.

mod action;
mod key;
mod resolver;

pub use action::Action;
pub use key::{Key, display_keys, parse_keys};
pub use resolver::{Layers, Outcome, Resolver};

use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Context {
    Global,
    Board,
    Detail,
    Help,
    /// The picker in normal mode: `j`/`k` move.
    Picker,
    /// The picker while typing. `picker` bindings with a modifier apply
    /// here too, so `<C-t>` works in both modes.
    PickerInsert,
    /// The `:` and `/` lines.
    Command,
    /// A range of cards on the board, for one grouped action. Board
    /// bindings apply too: these are only the ones it changes.
    Visual,
    /// An issue's pictures, full screen.
    Viewer,
}

impl Context {
    pub const ALL: [Context; 9] = [
        Context::Global,
        Context::Board,
        Context::Detail,
        Context::Help,
        Context::Picker,
        Context::PickerInsert,
        Context::Command,
        Context::Visual,
        Context::Viewer,
    ];

    /// The section name under `[keys]`, and the heading on the help page.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Context::Global => ("global", "Everywhere"),
            Context::Board => ("board", "Board"),
            Context::Detail => ("detail", "Issue"),
            Context::Help => ("help", "Help"),
            Context::Picker => ("picker", "Picker"),
            Context::PickerInsert => ("picker_insert", "Picker, typing"),
            Context::Command => ("command", "Command line"),
            Context::Visual => ("visual", "Visual (a range of cards)"),
            Context::Viewer => ("viewer", "Pictures"),
        }
    }

    pub fn name(self) -> &'static str {
        self.names().0
    }

    pub fn title(self) -> &'static str {
        self.names().1
    }

    /// Whether this context is a text field, where keys type.
    pub fn is_text(self) -> bool {
        matches!(self, Context::PickerInsert | Context::Command)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub keys: Vec<Key>,
    pub action: Action,
}

use Action as A;
use Context as C;

/// The defaults, in vim's spirit. Each line is one binding; the help page
/// is generated from this table (and whatever the config changed).
const DEFAULTS: &[(Context, &str, Action)] = &[
    (C::Global, "q", A::Close),
    (C::Global, "<Esc>", A::Close),
    (C::Global, "<C-c>", A::Quit),
    (C::Global, "?", A::Help),
    (C::Global, ":", A::CommandLine),
    (C::Global, "<leader>f", A::PickerIssues),
    (C::Global, "<leader>F", A::PickerRemote),
    (C::Global, "<leader>b", A::PickerBoards),
    (C::Global, "<leader>p", A::PickerCommands),
    (C::Global, "<leader>t", A::PickerThemes),
    (C::Global, "<leader>'", A::PickerResume),
    (C::Global, "r", A::Refresh),
    (C::Board, "h", A::Left),
    (C::Board, "l", A::Right),
    (C::Board, "j", A::Down),
    (C::Board, "k", A::Up),
    (C::Board, "gg", A::Top),
    (C::Board, "G", A::Bottom),
    (C::Board, "0", A::FirstColumn),
    (C::Board, "$", A::LastColumn),
    (C::Board, "<C-d>", A::HalfPageDown),
    (C::Board, "<C-u>", A::HalfPageUp),
    (C::Board, "H", A::MoveCardLeft),
    (C::Board, "L", A::MoveCardRight),
    (C::Board, "J", A::MoveCardDown),
    (C::Board, "K", A::MoveCardUp),
    (C::Board, "<CR>", A::OpenDetail),
    (C::Board, "gd", A::OpenDetail),
    (C::Board, "gp", A::OpenParent),
    (C::Board, "/", A::Filter),
    (C::Board, "n", A::NextMatch),
    (C::Board, "N", A::PrevMatch),
    // Not `close`: Esc out of habit must not quit the app.
    (C::Board, "<Esc>", A::ClearFilter),
    (C::Board, "t", A::Transition),
    (C::Board, "<leader>a", A::Assign),
    (C::Board, "<leader>A", A::AssignMe),
    (C::Board, "<leader>u", A::PickerUsers),
    (C::Board, "c", A::Create),
    (C::Board, "C", A::Clone),
    (C::Board, "e", A::Edit),
    (C::Board, "i", A::Comment),
    (C::Board, "o", A::OpenBrowser),
    (C::Board, "yy", A::CopyKey),
    (C::Board, "yu", A::CopyUrl),
    (C::Board, "v", A::VisualMode),
    (C::Board, "s", A::ToggleSwimlanes),
    (C::Board, "ze", A::ToggleEmptyColumns),
    (C::Detail, "j", A::Down),
    (C::Detail, "k", A::Up),
    (C::Detail, "gg", A::Top),
    (C::Detail, "G", A::Bottom),
    (C::Detail, "<C-d>", A::HalfPageDown),
    (C::Detail, "<C-u>", A::HalfPageUp),
    (C::Detail, "<Tab>", A::NextTab),
    (C::Detail, "<S-Tab>", A::PrevTab),
    (C::Detail, "t", A::Transition),
    (C::Detail, "<leader>a", A::Assign),
    (C::Detail, "<leader>A", A::AssignMe),
    (C::Detail, "e", A::Edit),
    (C::Detail, "E", A::EditField),
    (C::Detail, "gi", A::ViewPictures),
    (C::Detail, "gp", A::OpenParent),
    (C::Viewer, "h", A::Left),
    (C::Viewer, "l", A::Right),
    (C::Viewer, "o", A::OpenBrowser),
    (C::Viewer, "yu", A::CopyUrl),
    (C::Detail, "i", A::Comment),
    (C::Detail, "o", A::OpenBrowser),
    (C::Detail, "yy", A::CopyKey),
    (C::Detail, "yu", A::CopyUrl),
    (C::Help, "j", A::Down),
    (C::Help, "k", A::Up),
    (C::Help, "gg", A::Top),
    (C::Help, "G", A::Bottom),
    (C::Help, "<C-d>", A::HalfPageDown),
    (C::Help, "<C-u>", A::HalfPageUp),
    (C::Picker, "<CR>", A::Confirm),
    (C::Picker, "j", A::Down),
    (C::Picker, "k", A::Up),
    (C::Picker, "gg", A::Top),
    (C::Picker, "G", A::Bottom),
    (C::Picker, "i", A::InsertMode),
    (C::Picker, "a", A::InsertMode),
    (C::Picker, "q", A::Close),
    (C::Picker, "<Esc>", A::Close),
    (C::Picker, "<C-c>", A::Close),
    (C::Picker, "<C-n>", A::Down),
    (C::Picker, "<C-p>", A::Up),
    (C::Picker, "<C-j>", A::Down),
    (C::Picker, "<C-k>", A::Up),
    (C::Picker, "<Down>", A::Down),
    (C::Picker, "<Up>", A::Up),
    (C::Picker, "<C-Space>", A::ToggleSource),
    (C::Picker, "<Tab>", A::ToggleMark),
    (C::Picker, "<C-d>", A::PreviewDown),
    (C::Picker, "<C-u>", A::PreviewUp),
    (C::Picker, "<C-t>", A::Transition),
    (C::Picker, "<C-a>", A::Assign),
    (C::Picker, "<C-o>", A::OpenBrowser),
    (C::Picker, "<C-y>", A::CopyKey),
    (C::Picker, "<C-r>", A::HistoryPrev),
    (C::PickerInsert, "<Esc>", A::NormalMode),
    (C::PickerInsert, "<Up>", A::HistoryPrev),
    (C::PickerInsert, "<Down>", A::HistoryNext),
    (C::Visual, "v", A::VisualMode),
    (C::Visual, "<Esc>", A::NormalMode),
    (C::Command, "<CR>", A::Confirm),
    (C::Command, "<Esc>", A::Close),
    (C::Command, "<C-c>", A::Close),
    (C::Command, "<Tab>", A::Down),
    (C::Command, "<S-Tab>", A::Up),
    (C::Command, "<C-n>", A::Down),
    (C::Command, "<C-p>", A::Up),
    (C::Command, "<C-y>", A::AcceptProposal),
    (C::Command, "<C-e>", A::HideProposals),
];

/// `[keys]` in the config. Every other table under it is a context:
/// `[keys.board]`, `[keys.picker]`... mapping a sequence to an action name,
/// or to `"none"` to unbind a default.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub leader: Option<String>,
    pub timeoutlen_ms: Option<u64>,
    /// In file order (`toml`'s `preserve_order`).
    #[serde(flatten)]
    pub sections: toml::Table,
}

pub struct Keymap {
    pub leader: Key,
    pub timeout: Duration,
    tables: BTreeMap<Context, Vec<Binding>>,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(600);

impl Keymap {
    pub fn defaults() -> Self {
        Self::from_config(&KeysConfig::default()).0
    }

    /// Builds the table, reporting every problem rather than stopping at
    /// the first: an unknown action, a key that does not parse, one key
    /// spelt two ways in one section. A bad line is skipped; the rest
    /// still applies.
    pub fn from_config(config: &KeysConfig) -> (Self, Vec<String>) {
        let mut problems = vec![];
        let space = Key::char(' ');
        let leader = match config.leader.as_deref().map(|l| parse_keys(l, space)) {
            None => space,
            Some(Ok(keys)) if keys.len() == 1 => keys[0],
            Some(_) => {
                problems.push(format!(
                    "keys.leader: `{}` is not a single key; using <Space>",
                    config.leader.as_deref().unwrap_or_default()
                ));
                space
            }
        };
        let mut tables: BTreeMap<Context, Vec<Binding>> = BTreeMap::new();
        for &(context, keys, action) in DEFAULTS {
            let keys = parse_keys(keys, leader).expect("the default bindings parse");
            tables.entry(context).or_default().push(Binding { keys, action });
        }
        for (section, bindings) in &config.sections {
            let Some(bindings) = bindings.as_table() else {
                problems.push(format!("keys.{section}: expected a table of bindings"));
                continue;
            };
            let Some(context) = Context::ALL.into_iter().find(|c| c.name() == section) else {
                let names: Vec<_> = Context::ALL.iter().map(|c| c.name()).collect();
                problems.push(format!(
                    "[keys.{section}]: no such context (one of {})",
                    names.join(", ")
                ));
                continue;
            };
            apply_section(
                tables.entry(context).or_default(),
                context,
                bindings,
                leader,
                &mut problems,
            );
        }
        let timeout = config.timeoutlen_ms.map(Duration::from_millis).unwrap_or(DEFAULT_TIMEOUT);
        (Keymap { leader, timeout, tables }, problems)
    }

    pub fn table(&self, context: Context) -> &[Binding] {
        self.tables.get(&context).map(Vec::as_slice).unwrap_or_default()
    }

    /// Each action of a context with every sequence bound to it, in the
    /// order actions are declared: the rows of the help page.
    pub fn help_rows(&self, context: Context) -> Vec<(String, Action)> {
        let mut rows: Vec<(String, Action)> = vec![];
        let mut actions: Vec<Action> = self.table(context).iter().map(|b| b.action).collect();
        actions.sort();
        actions.dedup();
        for action in actions {
            let keys: Vec<String> = self
                .table(context)
                .iter()
                .filter(|b| b.action == action)
                .map(|b| display_keys(&b.keys))
                .collect();
            rows.push((keys.join(" "), action));
        }
        rows
    }

    /// The first sequence bound to an action anywhere, for the command
    /// palette.
    pub fn keys_for(&self, action: Action) -> Option<String> {
        self.tables.values().flatten().find(|b| b.action == action).map(|b| display_keys(&b.keys))
    }
}

fn apply_section(
    table: &mut Vec<Binding>,
    context: Context,
    bindings: &toml::Table,
    leader: Key,
    problems: &mut Vec<String>,
) {
    let mut seen: Vec<(Vec<Key>, &str)> = vec![];
    for (spelling, value) in bindings {
        let at = format!("[keys.{}] \"{spelling}\"", context.name());
        let (keys, action) = match read_binding(spelling, value, context, leader) {
            Ok(binding) => binding,
            Err(e) => {
                problems.push(format!("{at}: {e}"));
                continue;
            }
        };
        if let Some((_, first)) = seen.iter().find(|(k, _)| *k == keys) {
            problems.push(format!("{at}: the same key as \"{first}\", which is kept"));
            continue;
        }
        seen.push((keys.clone(), spelling));
        table.retain(|b| b.keys != keys);
        if let Some(action) = action {
            table.push(Binding { keys, action });
        }
    }
}

/// One `"keys" = "action"` line. `None` for the action unbinds.
fn read_binding(
    spelling: &str,
    value: &toml::Value,
    context: Context,
    leader: Key,
) -> Result<(Vec<Key>, Option<Action>), String> {
    let name = value.as_str().ok_or("expected an action name in quotes")?;
    let keys = parse_keys(spelling, leader)?;
    if context.is_text() && keys[0].as_text().is_some() {
        return Err("would type a character here; give it a modifier".into());
    }
    match name {
        "none" => Ok((keys, None)),
        name => match Action::from_name(name) {
            Some(action) => Ok((keys, Some(action))),
            None => Err(format!("unknown action `{name}`")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml_src: &str) -> KeysConfig {
        toml::from_str(toml_src).unwrap()
    }

    fn bound(map: &Keymap, context: Context, keys: &str) -> Option<Action> {
        let keys = parse_keys(keys, map.leader).unwrap();
        map.table(context).iter().find(|b| b.keys == keys).map(|b| b.action)
    }

    #[test]
    fn the_defaults_parse_without_a_conflict() {
        let (map, problems) = Keymap::from_config(&KeysConfig::default());
        assert!(problems.is_empty(), "{problems:?}");
        for context in Context::ALL {
            let table = map.table(context);
            for (i, b) in table.iter().enumerate() {
                assert!(
                    !table[..i].iter().any(|o| o.keys == b.keys),
                    "{} bound twice in {}",
                    display_keys(&b.keys),
                    context.name()
                );
            }
        }
        assert_eq!(bound(&map, Context::Board, "L"), Some(Action::MoveCardRight));
        assert_eq!(map.timeout, Duration::from_millis(600));
    }

    #[test]
    fn the_config_rebinds_unbinds_and_moves_the_leader() {
        let (map, problems) = Keymap::from_config(&config(
            r#"
            leader = ","
            timeoutlen_ms = 300
            [board]
            "J" = "move_card_right"
            "L" = "none"
            [picker]
            "<C-x>" = "transition"
            "#,
        ));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(map.leader, Key::char(','));
        assert_eq!(map.timeout, Duration::from_millis(300));
        assert_eq!(bound(&map, Context::Board, "J"), Some(Action::MoveCardRight));
        assert_eq!(bound(&map, Context::Board, "L"), None);
        assert_eq!(bound(&map, Context::Global, ",f"), Some(Action::PickerIssues));
        assert_eq!(bound(&map, Context::Picker, "<C-x>"), Some(Action::Transition));
    }

    #[test]
    fn every_mistake_is_reported_and_the_rest_still_applies() {
        let (map, problems) = Keymap::from_config(&config(
            r#"
            leader = "ab"
            [board]
            "<C-d>" = "half_page_down"
            "<c-d>" = "top"
            "<Foo>" = "top"
            "x" = "move_card_rigth"
            "X" = "clone"
            [boards]
            "x" = "top"
            [picker_insert]
            "j" = "down"
            "<C-j>" = 3
            "#,
        ));
        assert_eq!(
            problems,
            [
                "keys.leader: `ab` is not a single key; using <Space>",
                "[keys.board] \"<c-d>\": the same key as \"<C-d>\", which is kept",
                "[keys.board] \"<Foo>\": `<Foo>` is not a key",
                "[keys.board] \"x\": unknown action `move_card_rigth`",
                "[keys.boards]: no such context (one of global, board, detail, help, picker, picker_insert, command, visual, viewer)",
                "[keys.picker_insert] \"j\": would type a character here; give it a modifier",
                "[keys.picker_insert] \"<C-j>\": expected an action name in quotes",
            ]
        );
        assert_eq!(bound(&map, Context::Board, "X"), Some(Action::Clone));
        assert_eq!(bound(&map, Context::Board, "<C-d>"), Some(Action::HalfPageDown));
    }

    #[test]
    fn help_rows_follow_the_table() {
        let (map, _) = Keymap::from_config(&config("[board]\n\"J\" = \"move_card_right\""));
        let rows = map.help_rows(Context::Board);
        assert!(rows.contains(&("L J".into(), Action::MoveCardRight)), "{rows:?}");
        assert!(rows.contains(&("<CR> gd".into(), Action::OpenDetail)));
        assert_eq!(map.keys_for(Action::Help).as_deref(), Some("?"));
    }
}
