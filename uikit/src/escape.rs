//! A window's escape button (#892): the button Esc presses, as a dialog's
//! Cancel. An app marks one button's action with [`ESCAPE`]
//! ([`escape_action`]). The mark is a key binding only. The press is the
//! button's own, carrying the action the button keeps, and the mark itself
//! never reaches the accessibility tree's bound actions, so no agent row,
//! tier or consent ever sees it as something to call.
//!
//! Esc goes to it only when nothing else takes Esc: an open menu closes, and
//! a focused code view gets the key. `desktop`'s trusted card takes every key
//! while it is up, so no app sees Esc then.

use alloc::format;
use alloc::string::String;

/// The mark on a button's action: Esc presses it.
pub const ESCAPE: &str = "kai:escape";

/// A button's action marked as the window's escape button, keeping `action`,
/// the one its press runs, if any.
pub fn escape_action(action: Option<&str>) -> String {
    match action {
        Some(action) => format!("{ESCAPE} {action}"),
        None => String::from(ESCAPE),
    }
}

/// Whether a button's `action` carries the escape mark, and the action it
/// keeps without the mark.
pub fn split(action: Option<&str>) -> (bool, Option<&str>) {
    let Some(action) = action else {
        return (false, None);
    };
    if action == ESCAPE {
        return (true, None);
    }
    match action
        .strip_prefix(ESCAPE)
        .and_then(|rest| rest.strip_prefix(' '))
    {
        Some(kept) => (true, Some(kept)),
        None => (false, Some(action)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_comes_off_and_the_action_stays() {
        assert_eq!(split(Some(&escape_action(None))), (true, None));
        assert_eq!(
            split(Some(&escape_action(Some("shell_stop")))),
            (true, Some("shell_stop"))
        );
        assert_eq!(split(Some("notes_add")), (false, Some("notes_add")));
        // Only the mark, then a space: a name that starts like it is not one.
        assert_eq!(split(Some("kai:escaped")), (false, Some("kai:escaped")));
        assert_eq!(split(None), (false, None));
    }
}
