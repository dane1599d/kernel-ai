//! P2's file rows (task 11; ADR 0002's P2 row, ADR 0004 §6), owned by the `fs`
//! server. Paths are under `/home/user`: a relative path starts there, and the
//! server's lookup is path-beneath, so `..` cannot leave it (P2 plan §4.3).
//!
//! A move and an overwrite you plainly ask for run at once with undo; the AI's
//! own pick asks, as any added change does (ADR 0002 question 9, decided
//! 2026-10-27). They are in [`crate::ROWS`], so the pack lists them under
//! `## files`, now that the shell is wired to the `fs` server.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

/// A path under `/home/user`, named in the request.
const PATH: Arg = Arg {
    from_request: true,
    ..arg("path", ArgType::Text { max: 255 }, Role::Target)
};

/// Text to write: at most one `kai-fs 1` message of data (`fsproto::MAX_DATA`).
const TEXT: Arg = arg("text", ArgType::Text { max: 7 * 1024 }, Role::Payload);

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

const STRING: RetType = RetType::Of(ArgType::Text { max: 64 });

/// A read of the user's files: what it returns is the files' content, data.
const fn read(name: &'static str, doc: &'static str, args: &'static [Arg]) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_READ,
        inverse: Inverse::Read,
        cues: &[],
        returns: Returns::Content("fs"),
        ret: RetType::Data,
        owner: Owner::Service("fs"),
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// A change `undo` puts back: the `fs` server restores the transaction's
/// snapshot, and refuses one over a later change (compare-and-set).
const fn change(name: &'static str, doc: &'static str, args: &'static [Arg]) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_CHANGE,
        inverse: Inverse::Snapshot,
        cues: &[],
        returns: Returns::System,
        ret: RetType::Unit,
        owner: Owner::Service("fs"),
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

pub const FS_LIST: Action = Action {
    cues: &["folder", "files"],
    ..read("fs_list", "a folder's names", &[PATH])
};

pub const FS_READ: Action = Action {
    cues: &["read", "cat"],
    ..read("fs_read", "a file's text", &[PATH])
};

pub const FS_HISTORY: Action = Action {
    cues: &["history", "versions"],
    ..read("fs_history", "a file's earlier versions", &[PATH])
};

pub const FS_CHANGES: Action = Action {
    cues: &["changed", "changes"],
    returns: Returns::System,
    ret: STRING,
    ..read("fs_changes", "files changed today", &[])
};

pub const FS_CREATE: Action = Action {
    cues: &["create"],
    ..change("fs_create", "a new file; never overwrites", &[PATH, TEXT])
};

pub const FS_WRITE: Action = Action {
    cues: &["write", "overwrite"],
    ..change("fs_write", "a file's text, replaced", &[PATH, TEXT])
};

const TO: Arg = Arg { name: "to", ..PATH };

pub const FS_MOVE: Action = Action {
    cues: &["move", "rename"],
    ..change(
        "fs_move",
        "moves or renames",
        &[
            Arg {
                name: "from",
                ..PATH
            },
            TO,
        ],
    )
};

pub const FS_TRASH: Action = Action {
    cues: &["trash", "delete"],
    inverse: Inverse::Trash,
    ..change("fs_trash", "to the trash; undo brings it back", &[PATH])
};

pub const FS_RESTORE: Action = Action {
    cues: &["restore", "recover"],
    ..change(
        "fs_restore",
        "an earlier version, or out of the trash",
        &[PATH],
    )
};

/// What the trash held is gone for good, so it asks once.
pub const FS_EMPTY_TRASH: Action = Action {
    cues: &["empty"],
    facts: Facts {
        destructive: true,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::None("emptied files cannot come back"),
    ..change("fs_empty_trash", "no undo", &[])
};

/// Every file row, in the order the pack will list them.
pub const FS_ROWS: [Action; 10] = [
    FS_LIST,
    FS_READ,
    FS_HISTORY,
    FS_CHANGES,
    FS_CREATE,
    FS_WRITE,
    FS_MOVE,
    FS_TRASH,
    FS_RESTORE,
    FS_EMPTY_TRASH,
];

const _: () = assert!(crate::check(&FS_ROWS).is_ok(), "a file row is inconsistent");

/// What a model's script may call: the script API, then the file rows, so a
/// script's file changes are one transaction (P2 task 11).
pub const SCRIPT_CALLS: [Action; crate::SCRIPT_API.len() + FS_ROWS.len()] = {
    let api = crate::SCRIPT_API;
    let mut out = [api[0]; crate::SCRIPT_API.len() + FS_ROWS.len()];
    let mut i = 0;
    while i < out.len() {
        out[i] = match i < api.len() {
            true => api[i],
            false => FS_ROWS[i - api.len()],
        };
        i += 1;
    }
    out
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn plain_file_changes_run_with_undo_and_only_emptying_the_trash_asks() {
        for row in &FS_ROWS {
            let want = match row.name {
                "fs_empty_trash" => Tier::Ask,
                _ => Tier::Run,
            };
            assert_eq!(derive(row), want, "{}", row.name);
        }
    }

    #[test]
    fn file_rows_are_os_rows_now_that_fs_is_wired() {
        for row in &FS_ROWS {
            assert!(
                crate::ROWS.iter().any(|r| r.name == row.name),
                "{}",
                row.name
            );
        }
    }

    #[test]
    fn what_a_file_holds_is_data() {
        for row in [FS_LIST, FS_READ, FS_HISTORY] {
            assert!(matches!(row.returns, Returns::Content(_)), "{}", row.name);
        }
    }
}
