//! The agent's rows for app windows (ADR 0005 §6, P3 task 10): `desktop` owns
//! them, scripts call them by the same names. Declared app actions
//! (`<app>_<function>`) come first; `ui_view` then `ui_act` is for a control
//! with no declared action.
//!
//! - `ui_windows` and `ui_view` read. A view is `Data` (ADR 0002 D6): what an
//!   app puts in its tree cannot start an action, add one or lower a tier.
//! - `ui_open`, `ui_focus` and `window_close` run when asked and are undone by
//!   their inverses. `window_close` asks when the app may have unsaved work,
//!   which the shell decides from `desktop`'s answer, as `app_mem_limit`'s raise
//!   is decided against the quota in force.
//! - `ui_act` takes the tier of the declared action its node is bound to; an
//!   unbound node asks, so the row's own tier is ask.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

const fn arg(name: &'static str, ty: ArgType, role: Role) -> Arg {
    Arg {
        name,
        ty,
        role,
        from_request: false,
        never: &[],
        aliases: &[],
    }
}

/// A window: its app's name as the State line's `windows` gives it (`notes`),
/// or its id from `ui_windows` or a view (`w3`). For consent it must be in the
/// user's line, so "switch to notes" runs `ui_focus("notes")` at once, while a
/// window the model picked by id asks.
const WINDOW: Arg = Arg {
    from_request: true,
    ..arg("window", ArgType::Text { max: 24 }, Role::Target)
};
/// A node's short id in the window's last view: `b2`.
const NODE: Arg = arg("node", ArgType::Text { max: 8 }, Role::Target);
/// An app named in the request: a package name (ADR 0011 §2).
const APP: Arg = Arg {
    from_request: true,
    ..arg("app", ArgType::Text { max: 24 }, Role::Target)
};
/// What the app opens with, such as a path.
const OPEN_ARG: Arg = arg(
    "arg",
    ArgType::Opt(&ArgType::Text { max: 256 }),
    Role::Payload,
);

/// AccessKit 0.25.1's actions as `ui_act` names them, in the order of their
/// bits in `surfabi::tree::action`.
pub const UI_ACTIONS: &[&str] = &[
    "click",
    "focus",
    "set_value",
    "increment",
    "decrement",
    "expand",
    "collapse",
    "scroll_into_view",
    "show_context_menu",
];

/// The bit number `surfabi::tree::action` gives `action`.
pub fn ui_action_bit(action: &str) -> Option<u32> {
    UI_ACTIONS
        .iter()
        .position(|a| *a == action)
        .map(|n| n as u32)
}

const DESKTOP: Owner = Owner::Service("desktop");

const fn read(name: &'static str, doc: &'static str, args: &'static [Arg]) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_READ,
        inverse: Inverse::Read,
        cues: &[],
        returns: Returns::System,
        ret: RetType::Unit,
        owner: DESKTOP,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

const fn change(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    inverse: Inverse,
) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_CHANGE,
        inverse,
        cues: &[],
        returns: Returns::System,
        ret: RetType::Unit,
        owner: DESKTOP,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

const TEXT: RetType = RetType::Of(ArgType::Text { max: 64 });

/// The open windows, front to back: the focused one first when a window has
/// focus.
pub const UI_WINDOWS: Action = Action {
    cues: &["windows"],
    ret: RetType::List(&RetType::Record(&[
        ("id", TEXT),
        ("app", TEXT),
        ("title", TEXT),
    ])),
    ..read("ui_windows", "the open windows, front to back", &[])
};

/// The window list and `window`'s controls with their short ids, at most 300
/// tokens: `Data`, read in a second round.
pub const UI_VIEW: Action = Action {
    cues: &["look", "see", "view", "show"],
    returns: Returns::Content("ui"),
    ret: RetType::Data,
    ..read("ui_view", "a window's controls and their ids", &[WINDOW])
};

/// Opens a native app; `undo` closes the window it opened.
pub const UI_OPEN: Action = Action {
    cues: &["open", "launch", "start", "run"],
    ..change(
        "ui_open",
        "opens an app, with a path or text for it",
        &[APP, OPEN_ARG],
        Inverse::Compensate("window_close"),
    )
};

/// Moves keyboard focus; `undo` puts it back.
pub const UI_FOCUS: Action = Action {
    cues: &["focus", "switch", "bring", "go"],
    ..change(
        "ui_focus",
        "moves keyboard focus to a window",
        &[WINDOW],
        Inverse::Restore,
    )
};

/// Closes any window, a Linux one too (ADR 0006); `undo` reopens it. Asks when
/// the app may have unsaved work.
pub const WINDOW_CLOSE: Action = Action {
    cues: &["close", "quit", "exit"],
    ..change(
        "window_close",
        "closes a window; asks if work may be unsaved",
        &[WINDOW],
        Inverse::Compensate("ui_open"),
    )
};

/// Sends an AccessKit action to a node of the window's last view, never
/// synthetic input.
pub const UI_ACT: Action = Action {
    name: "ui_act",
    doc: "acts on a control by its id in the view",
    args: &[
        WINDOW,
        NODE,
        arg("action", ArgType::Enum(UI_ACTIONS), Role::Option),
        arg(
            "value",
            ArgType::Opt(&ArgType::Text { max: 512 }),
            Role::Payload,
        ),
    ],
    // What an app's control does is the app's: it may delete, and has no undo
    // the shell knows of.
    facts: Facts {
        destructive: true,
        idempotent: false,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::None("the app's own control"),
    cues: &["click", "press", "type", "fill", "set", "enter", "select"],
    returns: Returns::System,
    ret: RetType::Unit,
    owner: DESKTOP,
    needs: 0,
    min_tier: Tier::Ask,
    old_name: None,
};

/// Every window row, in the pack's order.
pub const UI_ROWS: [Action; 6] = [UI_WINDOWS, UI_VIEW, UI_OPEN, UI_FOCUS, WINDOW_CLOSE, UI_ACT];

const _: () = assert!(
    crate::check(&UI_ROWS).is_ok(),
    "a window row is inconsistent"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn the_tiers_are_adr_0005s() {
        for row in &UI_ROWS {
            let want = match row.name {
                "ui_act" => Tier::Ask,
                _ => Tier::Run,
            };
            assert_eq!(derive(row), want, "{}", row.name);
        }
    }

    #[test]
    fn each_change_is_undone_by_its_partner() {
        assert!(matches!(
            UI_OPEN.inverse,
            Inverse::Compensate("window_close")
        ));
        assert!(matches!(
            WINDOW_CLOSE.inverse,
            Inverse::Compensate("ui_open")
        ));
        assert!(matches!(UI_FOCUS.inverse, Inverse::Restore));
        assert!(matches!(UI_VIEW.ret, RetType::Data));
    }

    #[test]
    fn action_names_are_surfabis_bits() {
        assert_eq!(ui_action_bit("click"), Some(0));
        assert_eq!(ui_action_bit("set_value"), Some(2));
        assert_eq!(ui_action_bit("show_context_menu"), Some(8));
        assert_eq!(ui_action_bit("type"), None);
    }
}
