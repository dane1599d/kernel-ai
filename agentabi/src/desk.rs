//! P3's app lifecycle and resource-profile rows (P3 plan tasks 11 and 12; the
//! founder's decisions of 2026-10-27). A standing instruction's action is one of
//! these: `on minimize(notes) -> app_suspend(notes) until restore(notes)`.
//!
//! They are not in [`crate::ROWS`] yet: the pack renders them once the desktop's
//! events, comphost's suspend and resume, and the kernel's profile floors are
//! wired. The kernel enforces a profile's floors, never the shell.

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

/// An app named in the request: a package name (ADR 0011 §2).
const APP: Arg = Arg {
    from_request: true,
    ..arg("app", ArgType::Text { max: 12 }, Role::Target)
};

/// A profile named in the request: `gaming`, `quiet`, one the user made.
const PROFILE: Arg = Arg {
    from_request: true,
    ..arg("name", ArgType::Text { max: 24 }, Role::Target)
};

/// A change the owner puts back, run at once when plainly asked.
const fn change(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    inverse: Inverse,
    owner: Owner,
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
        owner,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// Saves an app through `kai:live`'s `save`, drops its instance and frees its
/// memory; `app_resume` brings it back. A suspend that cannot save does not happen.
pub const APP_SUSPEND: Action = Action {
    cues: &["suspend", "pause", "freeze", "sleep"],
    ..change(
        "app_suspend",
        "saves an app and frees its memory",
        &[APP],
        Inverse::Compensate("app_resume"),
        Owner::Service("comphost"),
    )
};

pub const APP_RESUME: Action = Action {
    cues: &["resume", "wake", "unfreeze", "reopen"],
    ..change(
        "app_resume",
        "brings a suspended app back as it was",
        &[APP],
        Inverse::Compensate("app_suspend"),
        Owner::Service("comphost"),
    )
};

pub const PROFILE_LIST: Action = Action {
    name: "profile_list",
    doc: "the resource profiles",
    args: &[],
    facts: Facts::LOCAL_READ,
    inverse: Inverse::Read,
    cues: &["profiles", "modes"],
    returns: Returns::System,
    ret: RetType::Of(ArgType::Text { max: 64 }),
    owner: Owner::Kernel,
    needs: 0,
    min_tier: Tier::Run,
    old_name: None,
};

/// Switches between existing profiles; `undo` puts the previous one back.
pub const PROFILE_SET: Action = Action {
    cues: &["mode", "profile"],
    ..change(
        "profile_set",
        "switches the resource profile",
        &[PROFILE],
        Inverse::Restore,
        Owner::Kernel,
    )
};

/// A profile's text (`kai-profile 1`).
const PROFILE_TEXT: Arg = arg("text", ArgType::Text { max: 2048 }, Role::Payload);

/// Making or editing a profile changes what every app may use, so it asks.
pub const PROFILE_CREATE: Action = Action {
    cues: &["create", "make", "new"],
    min_tier: Tier::Ask,
    ..change(
        "profile_create",
        "a new resource profile; asks first",
        &[PROFILE, PROFILE_TEXT],
        Inverse::Restore,
        Owner::Kernel,
    )
};

pub const PROFILE_EDIT: Action = Action {
    name: "profile_edit",
    doc: "changes a resource profile; asks first",
    cues: &["edit", "change"],
    ..PROFILE_CREATE
};

/// M10's `classes` policy's classes, a CPU class for an app.
pub const CLASSES: &[&str] = &["interactive", "normal", "background", "firm"];

pub const APP_PRIORITY: Action = Action {
    cues: &["priority", "prioritize", "prioritise"],
    ..change(
        "app_priority",
        "an app's CPU class",
        &[APP, arg("class", ArgType::Enum(CLASSES), Role::Option)],
        Inverse::Restore,
        Owner::Kernel,
    )
};

/// Lowering runs at once; raising asks, which the shell decides against the
/// quota in force, since consent cannot know it (P3 plan task 12).
pub const APP_MEM_LIMIT: Action = Action {
    cues: &["memory", "limit", "ram"],
    ..change(
        "app_mem_limit",
        "an app's memory quota in pages; raising asks",
        &[
            APP,
            arg(
                "pages",
                ArgType::Int {
                    min: 1,
                    max: 1 << 20,
                },
                Role::Option,
            ),
        ],
        Inverse::Restore,
        Owner::Kernel,
    )
};

/// Every P3 lifecycle and profile row.
pub const DESK_ROWS: [Action; 8] = [
    APP_SUSPEND,
    APP_RESUME,
    PROFILE_LIST,
    PROFILE_SET,
    PROFILE_CREATE,
    PROFILE_EDIT,
    APP_PRIORITY,
    APP_MEM_LIMIT,
];

const _: () = assert!(crate::check(&DESK_ROWS).is_ok(), "a P3 row is inconsistent");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn the_tiers_are_the_plans() {
        for row in &DESK_ROWS {
            let want = match row.name {
                "profile_create" | "profile_edit" => Tier::Ask,
                _ => Tier::Run,
            };
            assert_eq!(derive(row), want, "{}", row.name);
        }
    }

    #[test]
    fn suspend_and_resume_undo_each_other_and_none_is_rendered_yet() {
        assert!(matches!(
            APP_SUSPEND.inverse,
            Inverse::Compensate("app_resume")
        ));
        assert!(matches!(
            APP_RESUME.inverse,
            Inverse::Compensate("app_suspend")
        ));
        for row in &DESK_ROWS {
            assert!(
                !crate::ROWS.iter().any(|r| r.name == row.name),
                "{}",
                row.name
            );
        }
    }
}
