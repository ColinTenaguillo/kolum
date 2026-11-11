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
    MoveCardDown "move_card_down" "Move the card down its column",
    MoveCardUp "move_card_up" "Move the card up its column",
    OpenDetail "open_detail" "Open the issue",
    OpenParent "open_parent" "Open the parent issue",
    OpenLinked "open_linked" "Open a linked or a child issue",
    AddLink "add_link" "Link the issue to another",
    RemoveLink "remove_link" "Remove one of the issue's links",
    JumpBack "jump_back" "Back to the issue left before",
    JumpForward "jump_forward" "Forward to the issue left after",
    Filter "filter" "Filter the board, or search the issue",
    NextMatch "next_match" "Next match",
    PrevMatch "prev_match" "Previous match",
    ClearFilter "clear_filter" "Clear the filter or the search",
    Transition "transition" "Change the status",
    Assign "assign" "Assign to someone",
    AssignMe "assign_me" "Assign to me",
    Create "create" "Create an issue",
    Clone "clone" "Clone the issue",
    Edit "edit" "Edit in $EDITOR",
    EditField "edit_field" "Edit one field in $EDITOR",
    ViewPictures "view_pictures" "The issue's pictures, full screen",
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
    PickerUsers "picker_users" "Show the cards of some people",
    Confirm "confirm" "Confirm the choice",
    AcceptProposal "accept_proposal" "Take the proposal, to see what can follow",
    HideProposals "hide_proposals" "Hide the proposals",
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

    /// Whether it changes an issue, which a read-only provider refuses.
    pub fn writes(self) -> bool {
        matches!(
            self,
            Action::Transition
                | Action::Assign
                | Action::AssignMe
                | Action::Comment
                | Action::Edit
                | Action::EditField
                | Action::Clone
                | Action::AddLink
                | Action::RemoveLink
        )
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
