//! The memory rows (ADR 0013 section 3; row M2 of `docs/plans/agents-os.md`):
//! what Kai keeps of what the person told it, over `kaimem`'s store. The shell
//! carries them out (`shellcore::memrows`):
//!
//! - `memory_remember` keeps the person's own words when their line says
//!   "remember …"; one the model proposes asks with the exact text; one read
//!   from content is refused, so a file or a page never plants one.
//! - `memory_edit` keeps a text the line wrote, and asks for any other.
//! - `memory_forget` and `memory_forget_all` can be undone for 10 minutes; then
//!   the memory is purged. Forgetting everything asks.
//!
//! They are owned by the `memory` service, so the pack lists them as one index
//! line (ADR 0010 §6); the shell intercepts them before any binding.

use crate::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};

const OWNER: Owner = Owner::Service("memory");

const fn arg(name: &'static str, ty: ArgType, role: Role, from_request: bool) -> Arg {
    Arg {
        name,
        ty,
        role,
        from_request,
        never: &[],
        aliases: &[],
    }
}

/// The memory, by its number in `memory_list`.
const ID: Arg = arg(
    "id",
    ArgType::Int {
        min: 1,
        max: 1 << 40,
    },
    Role::Target,
    false,
);
/// What is kept: `kaimem`'s 200 bytes.
const TEXT: Arg = arg("text", ArgType::Text { max: 200 }, Role::Payload, true);

const fn change(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    cues: &'static [&'static str],
    min_tier: Tier,
) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_CHANGE,
        inverse: Inverse::Restore,
        cues,
        returns: Returns::System,
        ret: RetType::Unit,
        owner: OWNER,
        needs: 0,
        min_tier,
        old_name: None,
    }
}

const fn read(
    name: &'static str,
    doc: &'static str,
    args: &'static [Arg],
    cues: &'static [&'static str],
) -> Action {
    Action {
        name,
        doc,
        args,
        facts: Facts::LOCAL_READ,
        inverse: Inverse::Read,
        cues,
        returns: Returns::System,
        ret: RetType::Of(ArgType::Text { max: 4096 }),
        owner: OWNER,
        needs: 0,
        min_tier: Tier::Run,
        old_name: None,
    }
}

/// `memory_remember(text)`: keeps a fact the person told Kai.
pub const MEMORY_REMEMBER: Action = change(
    "memory_remember",
    "keeps a fact the person told Kai",
    &[TEXT],
    &["remember"],
    Tier::Run,
);
/// `memory_list()`: what Kai keeps, newest first.
pub const MEMORY_LIST: Action = read(
    "memory_list",
    "what Kai remembers, numbered",
    &[],
    &["remember", "memories"],
);
/// `memory_search(query)`: the memories that share a word with `query`.
pub const MEMORY_SEARCH: Action = read(
    "memory_search",
    "the memories about query",
    &[arg("query", ArgType::Text { max: 64 }, Role::Option, false)],
    &["remember", "memories"],
);
/// `memory_edit(id, text)`: memory `id` says `text` now.
pub const MEMORY_EDIT: Action = change(
    "memory_edit",
    "changes what memory id says",
    &[ID, TEXT],
    &["change", "correct", "edit"],
    Tier::Run,
);
/// `memory_forget(id)`: memory `id` goes; `undo` brings it back for 10 minutes.
pub const MEMORY_FORGET: Action = change(
    "memory_forget",
    "forgets memory id",
    &[ID],
    &["forget"],
    Tier::Run,
);
/// `memory_forget_all()`: every memory goes. It asks; `undo` brings them back
/// for 10 minutes.
pub const MEMORY_FORGET_ALL: Action = Action {
    facts: Facts {
        destructive: true,
        ..Facts::LOCAL_CHANGE
    },
    ..change(
        "memory_forget_all",
        "forgets every memory; it asks first",
        &[],
        &["forget everything", "forget all"],
        Tier::Ask,
    )
};

/// The memory rows, in the pack's index as the `memory` group.
pub const MEMORY_ROWS: [Action; 6] = [
    MEMORY_REMEMBER,
    MEMORY_LIST,
    MEMORY_SEARCH,
    MEMORY_EDIT,
    MEMORY_FORGET,
    MEMORY_FORGET_ALL,
];

const _: () = assert!(
    crate::check(&MEMORY_ROWS).is_ok(),
    "a memory row is inconsistent"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn reads_and_undoable_changes_run_and_forgetting_everything_asks() {
        for row in [
            &MEMORY_REMEMBER,
            &MEMORY_LIST,
            &MEMORY_SEARCH,
            &MEMORY_EDIT,
            &MEMORY_FORGET,
        ] {
            assert_eq!(derive(row), Tier::Run, "{}", row.name);
        }
        assert_eq!(derive(&MEMORY_FORGET_ALL), Tier::Ask);
    }

    #[test]
    fn a_kept_text_must_be_the_lines_own() {
        assert!(MEMORY_REMEMBER.args[0].from_request);
        assert!(MEMORY_EDIT.args[1].from_request);
    }
}
