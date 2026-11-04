//! What the `:` line proposes: a command while its name is typed, then a
//! theme after `theme `.

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
        None => (
            0,
            COMMANDS
                .iter()
                .filter(|(name, _)| name.starts_with(line))
                .map(|&(name, action)| Proposal { text: name.into(), about: action.description() })
                .collect(),
        ),
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
    fn an_empty_line_proposes_every_command() {
        let (start, list) = texts("");
        assert_eq!((start, list.len()), (0, COMMANDS.len()));
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
