//! Everything a key can do, by name. The name is the only thing the
//! config knows: `"L" = "move_card_right"`.

macro_rules! actions {
    ($($variant:ident $name:literal $desc:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action { $($variant),* }

        impl Action {
            pub const ALL: &[Action] = &[$(Action::$variant),*];

            pub fn name(self) -> &'static str {
                match self { $(Action::$variant => $name),* }
            }

            pub fn description(self) -> &'static str {
                match self { $(Action::$variant => $desc),* }
            }
        }
    };
}

actions! {
    Quit "quit" "Quit kolum",
    Close "close" "Close the view, or quit from the board",
    Help "help" "Show the keys",
    CommandLine "command_line" "Command line",
    Refresh "refresh" "Reload the board",
    Left "left" "Previous column",
    Right "right" "Next column",
    Down "down" "Next card or line",
    Up "up" "Previous card or line",
    Top "top" "First card, or the [count]th",
    Bottom "bottom" "Last card, or the [count]th",
    FirstColumn "first_column" "First column",
    LastColumn "last_column" "Last column",
    HalfPageDown "half_page_down" "Half a page down",
    HalfPageUp "half_page_up" "Half a page up",
    MoveCardLeft "move_card_left" "Move the card to the column on the left",
    MoveCardRight "move_card_right" "Move the card to the column on the right",
    OpenDetail "open_detail" "Open the issue",
    Filter "filter" "Filter the board as you type",
    NextMatch "next_match" "Next card matching the filter",
    PrevMatch "prev_match" "Previous card matching the filter",
    ClearFilter "clear_filter" "Clear the filter",
    Transition "transition" "Change the status",
    Assign "assign" "Assign to someone",
    AssignMe "assign_me" "Assign to me",
    Create "create" "Create an issue",
    Clone "clone" "Clone the issue",
    Edit "edit" "Edit in $EDITOR",
    Comment "comment" "Add a comment",
    OpenBrowser "open_browser" "Open in the browser",
    CopyKey "copy_key" "Copy the key",
    CopyUrl "copy_url" "Copy the URL",
    VisualMode "visual_mode" "Select a range of cards, or stop",
    ToggleSwimlanes "toggle_swimlanes" "Cycle the swimlanes",
    ToggleEmptyColumns "toggle_empty_columns" "Show or hide empty columns",
    NextTab "next_tab" "Next tab",
    PrevTab "prev_tab" "Previous tab",
    PickerIssues "picker_issues" "Find an issue on the board",
    PickerRemote "picker_remote" "Search the provider",
    PickerBoards "picker_boards" "Switch board",
    PickerCommands "picker_commands" "Every action, with its keys",
    PickerThemes "picker_themes" "Switch theme",
    PickerResume "picker_resume" "Reopen the last picker",
    Confirm "confirm" "Confirm the choice",
    NormalMode "normal_mode" "Back to normal mode",
    InsertMode "insert_mode" "Back to typing",
    ToggleSource "toggle_source" "Switch between local and remote results",
    ToggleMark "toggle_mark" "Mark for a grouped action",
    PreviewDown "preview_down" "Scroll the preview down",
    PreviewUp "preview_up" "Scroll the preview up",
    HistoryPrev "history_prev" "Previous search",
    HistoryNext "history_next" "Next search",
}

impl Action {
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        let mut names: Vec<_> = Action::ALL.iter().map(|a| a.name()).collect();
        for a in Action::ALL {
            assert_eq!(Action::from_name(a.name()), Some(*a));
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Action::ALL.len());
        assert_eq!(Action::from_name("move_card_rigth"), None);
    }
}
