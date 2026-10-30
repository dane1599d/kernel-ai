//! Revoking what an app's effects may call (P4 task 7, ADR 0001): Settings'
//! "what each app can do" withholds a row an app's package declares for its
//! effects, and gives it back. The shell keeps the withheld pairs and refuses a
//! withheld row's effect as it refuses an undeclared one, from the next effect
//! on. Built-in imports and an app's own storage are not rows, so they are not
//! revocable in v0.1.
//!
//! The shell owns both rows, so they are not in the model's pack: Settings
//! calls them for a click, through the one door, logged with their undo.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

/// The app, by its package name (ADR 0011 §2).
const APP: Arg = Arg {
    name: "app",
    ty: ArgType::Text { max: 24 },
    role: Role::Target,
    from_request: true,
    never: &[],
    aliases: &[],
};
/// The row its effects call, as its package declares it.
const ROW: Arg = Arg {
    name: "row",
    ty: ArgType::Text { max: 24 },
    role: Role::Target,
    from_request: true,
    never: &[],
    aliases: &[],
};

const fn change(name: &'static str, doc: &'static str, inverse: Inverse, min_tier: Tier) -> Action {
    Action {
        name,
        doc,
        args: &[APP, ROW],
        facts: Facts::LOCAL_CHANGE,
        inverse,
        cues: &["revoke", "grant", "permission"],
        returns: Returns::System,
        ret: RetType::Unit,
        owner: Owner::Shell,
        needs: 0,
        min_tier,
        old_name: None,
    }
}

/// Withholds `row` from `app`'s effects; `undo` gives it back.
pub const GRANT_REVOKE: Action = change(
    "grant_revoke",
    "stops an app's effects calling a row",
    Inverse::Compensate("grant_restore"),
    Tier::Run,
);

/// Gives `row` back to `app`'s effects. It widens what an app may do, so it
/// asks; `undo` withholds it again.
pub const GRANT_RESTORE: Action = change(
    "grant_restore",
    "lets an app's effects call a row again; asks first",
    Inverse::Compensate("grant_revoke"),
    Tier::Ask,
);

/// Both grant rows.
pub const GRANT_ROWS: [Action; 2] = [GRANT_REVOKE, GRANT_RESTORE];

const _: () = assert!(
    crate::check(&GRANT_ROWS).is_ok(),
    "a grant row is inconsistent"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn revoking_runs_and_giving_back_asks() {
        assert_eq!(derive(&GRANT_REVOKE), Tier::Run);
        assert_eq!(derive(&GRANT_RESTORE), Tier::Ask);
    }

    #[test]
    fn each_is_undone_by_the_other() {
        assert!(matches!(
            GRANT_REVOKE.inverse,
            Inverse::Compensate("grant_restore")
        ));
        assert!(matches!(
            GRANT_RESTORE.inverse,
            Inverse::Compensate("grant_revoke")
        ));
    }
}
