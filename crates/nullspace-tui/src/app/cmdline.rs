use crate::action::Action;

use super::CmdlineState;

const COMMANDS: [&str; 11] = [
    "delete",
    "equations",
    "exit",
    "new",
    "openReference",
    "quantities",
    "rescan",
    "scan",
    "search",
    "tags",
    "trash",
];

pub fn command_matches(prefix: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| {
            command.starts_with(prefix) || command_matches_ignore_case(command, prefix)
        })
        .collect()
}

pub(super) fn selected_command(prefix: &str, selected: usize) -> Option<&'static str> {
    let matches = command_matches(prefix);
    matches
        .get(selected.min(matches.len().saturating_sub(1)))
        .copied()
}

pub(super) fn exact_command(input: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .find(|command| command.eq_ignore_ascii_case(input))
}

pub(super) fn command_action(command: &str) -> Option<Action> {
    match command {
        "delete" => Some(Action::DeleteRequest),
        "equations" => Some(Action::OpenEquations),
        "exit" => Some(Action::Quit),
        "new" => Some(Action::NewEquation),
        "openReference" => Some(Action::OpenReference),
        "quantities" => Some(Action::OpenQuantities),
        "rescan" => Some(Action::Rescan),
        "scan" => Some(Action::ScanOpen),
        "search" => Some(Action::StartSearch),
        "tags" => Some(Action::OpenTags),
        "trash" => Some(Action::OpenTrash),
        _ => None,
    }
}

pub(super) fn accept_cmdline_state(cmdline: &mut CmdlineState) {
    if let Some(command) = selected_command(&cmdline.input, cmdline.selected) {
        cmdline.input = command.to_string();
        cmdline.cursor = cmdline.input.len();
        cmdline.selected = 0;
    }
}

pub(super) fn cycle_cmdline_selection(cmdline: &mut CmdlineState, forward: bool) {
    let count = command_matches(&cmdline.input).len();
    if count == 0 {
        cmdline.selected = 0;
    } else if forward {
        cmdline.selected = (cmdline.selected + 1) % count;
    } else {
        cmdline.selected = cmdline.selected.checked_sub(1).unwrap_or(count - 1);
    }
}

fn command_matches_ignore_case(command: &str, prefix: &str) -> bool {
    prefix.len() <= command.len() && command[..prefix.len()].eq_ignore_ascii_case(prefix)
}
