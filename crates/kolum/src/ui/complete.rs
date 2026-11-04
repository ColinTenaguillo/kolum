//! What the `:` line proposes: a command or any action, by its name, while
//! it is typed; then a theme after `theme `.

use crate::keymap::Action;

/// The commands, described by the action each one runs. One that takes an
/// argument ends in a space, so taking it goes straight on to the argument.
const COMMANDS: &[(&str, Action)] = &[
    ("quit", Action::Quit),
    ("help", Action::Help),
    ("refresh", Action::Refresh),
    ("theme ", Action::PickerThemes),
    ("board ", Action::PickerBoards),
    ("jql ", Action::PickerRemote),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub text: String,
    pub about: &'static str,
}

/// The byte where the word being typed starts, and what can replace it.
pub fn proposals(line: &str, themes: &[String]) -> (usize, Vec<Proposal>) {
    match line.split_once(' ') {
        None => {
            let commands = COMMANDS.iter().map(|&(name, action)| (name, action));
            // Every action too, by its name, but once: `quit` is both.
            let actions = Action::ALL
                .iter()
                .filter(|a| !COMMANDS.iter().any(|(name, _)| name.trim_end() == a.name()))
                .map(|&a| (a.name(), a));
            let all = commands.chain(actions).filter(|(name, _)| name.starts_with(line));
            (
                0,
                all.map(|(name, a)| Proposal { text: name.into(), about: a.description() })
                    .collect(),
            )
        }
        Some(("theme", arg)) => (
            line.len() - arg.len(),
            themes
                .iter()
                .filter(|t| t.starts_with(arg))
                .map(|t| Proposal { text: t.clone(), about: "" })
                .collect(),
        ),
        Some(_) => (line.len(), vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str) -> (usize, Vec<String>) {
        let themes = ["gruvbox".to_string(), "kanagawa".into(), "kanagawa-dim".into()];
        let (start, list) = proposals(line, &themes);
        (start, list.into_iter().map(|p| p.text).collect())
    }

    #[test]
    fn an_empty_line_proposes_every_command_then_every_action() {
        let (start, list) = texts("");
        assert_eq!(start, 0);
        assert_eq!(list[..3], ["quit", "help", "refresh"], "the commands first");
        assert!(list.contains(&"view_pictures".to_string()), "then each action by its name");
        assert_eq!(list.iter().filter(|t| *t == "quit").count(), 1, "once, though both");
    }

    #[test]
    fn an_action_is_a_command_by_its_name() {
        assert_eq!(texts("open_p"), (0, vec!["open_parent".to_string()]));
        let (_, list) = proposals("open_p", &[]);
        assert_eq!(list[0].about, "Open the parent issue");
    }

    #[test]
    fn a_command_is_proposed_by_its_start_and_ends_in_a_space_when_it_takes_an_argument() {
        assert_eq!(texts("th"), (0, vec!["theme ".to_string()]));
        assert_eq!(texts("re"), (0, vec!["refresh".to_string()]));
        assert_eq!(texts("x"), (0, vec![]));
    }

    #[test]
    fn after_theme_the_themes_are_proposed() {
        assert_eq!(
            texts("theme "),
            (6, vec!["gruvbox".into(), "kanagawa".into(), "kanagawa-dim".into()])
        );
        assert_eq!(texts("theme kan"), (6, vec!["kanagawa".into(), "kanagawa-dim".into()]));
    }

    #[test]
    fn other_arguments_get_nothing() {
        assert_eq!(texts("board 12"), (8, vec![]));
    }
}
