//! The action registry (ADR 0002 D1): one row per action the AI may use, with its
//! arguments, what it touches, how it is undone and the least tier it runs at.
//! The script API (`llmproto::script::API`), the shell's tier table
//! (`shellcore::tiers::TIERS`) and its interim consent cues, the Rhai bindings and
//! `help`'s list of functions are all generated from these rows.
//!
//! A row's tier is never written by hand: [`derive`] computes it from the row's
//! facts and inverse, and `min_tier` can only raise it. [`check`] refuses an
//! inconsistent row, and runs at compile time over [`ROWS`].

#![no_std]

extern crate alloc;

pub mod consent;
mod desk;
mod files;
mod grants;
mod look;
mod names;
mod rows;
mod show;
mod ui;

pub use desk::*;
pub use files::*;
pub use grants::*;
pub use look::*;
pub use names::{RESERVED_GROUPS, is_reserved, takes_os_name};
pub use rows::*;
pub use show::{Signature, Type};
pub use ui::*;

/// Bumped when a row is renamed or a field changes meaning; old names stay bound for
/// one version through `old_name`.
pub const REGISTRY_VERSION: u32 = 1;
/// Longest action name, in bytes.
pub const MAX_NAME: usize = 24;
/// Longest one-line description for the model, in bytes. ADR 0002 D1 sets 60, so
/// that a `dsl-1` line fits ADR 0010's 120; version 1 keeps the M8 prompt byte for
/// byte, and `memory`'s line there is 63. Task 4's `dsl-1` renderer shortens it
/// and lowers this to 60.
pub const MAX_DOC: usize = 64;

/// One action the AI may use.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    /// `<group>_<verb>` in snake_case, at most [`MAX_NAME`] bytes, a valid Rhai and
    /// C identifier. The seven M8 names keep their names in version 1.
    pub name: &'static str,
    /// One line for the model, at most [`MAX_DOC`] bytes.
    pub doc: &'static str,
    pub args: &'static [Arg],
    pub facts: Facts,
    pub inverse: Inverse,
    /// Request stems that ask for it: a word of the user's line starting with one.
    pub cues: &'static [&'static str],
    /// What a read hands back, for provenance.
    pub returns: Returns,
    /// The result's shape.
    pub ret: RetType,
    /// Who executes it and its inverse.
    pub owner: Owner,
    /// The capability it needs (ADR 0001 turns these bits into handle kinds).
    pub needs: u64,
    /// The derived tier is at least this.
    pub min_tier: Tier,
    /// The name before a rename, bound too for one [`REGISTRY_VERSION`].
    pub old_name: Option<&'static str>,
}

/// One argument, in the order the action takes them.
#[derive(Clone, Copy, Debug)]
pub struct Arg {
    pub name: &'static str,
    pub ty: ArgType,
    pub role: Role,
    /// The value must appear in the user's line (or be an alias) for consent.
    pub from_request: bool,
    /// Values that make the call never-tier.
    pub never: &'static [&'static str],
    /// (what the user may write, the value it means).
    pub aliases: &'static [(&'static str, &'static str)],
}

/// The argument types the decoder can steer.
#[derive(Clone, Copy, Debug)]
pub enum ArgType {
    Enum(&'static [&'static str]),
    Int { min: i64, max: i64 },
    Text { max: u16 },
    Path,
    Host,
    List(&'static ArgType, u8),
    Opt(&'static ArgType),
}

/// The shape of a result.
#[derive(Clone, Copy, Debug)]
pub enum RetType {
    Unit,
    Of(ArgType),
    Record(&'static [(&'static str, RetType)]),
    List(&'static RetType),
    /// Content read from outside: an opaque value that cannot steer a risky call.
    Data,
}

/// What an argument is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// What the action acts on.
    Target,
    /// What goes in or out.
    Payload,
    /// How it does it.
    Option,
}

/// MCP's four hints. [`Facts::PESSIMISTIC`] is the default for anything not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl Facts {
    /// MCP's defaults: a change, destructive, not idempotent, open-world.
    pub const PESSIMISTIC: Self = Self {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true,
    };
    /// Reads system state on this machine only.
    pub const LOCAL_READ: Self = Self {
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    };
    /// A local change that does not destroy anything.
    pub const LOCAL_CHANGE: Self = Self {
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    };
}

/// How an action is undone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// It only reads; nothing to undo.
    Read,
    /// The owner puts back the state recorded before the change.
    Restore,
    /// Another action reverses it.
    Compensate(&'static str),
    /// A snapshot taken before the change is put back (P2).
    Snapshot,
    /// What it removes goes to the trash (P2).
    Trash,
    /// An outbound send held for `ms` with undo live (P5).
    Hold { ms: u32 },
    /// It cannot be undone, and why.
    None(&'static str),
}

/// Where a result comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Returns {
    /// System state: the OS's own values.
    System,
    /// Content read from a source, labelled `Data`.
    Content(&'static str),
}

/// Who executes an action and its inverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Kernel,
    /// The shell itself, for pure helpers.
    Shell,
    Service(&'static str),
    App(&'static str),
}

/// How a call is handled (ADR 0002 D2). Ordered: the strictest wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Runs at once: it only reads, or the shell records how to undo it.
    Run,
    /// Runs only after a yes: it has no undo, the model added it, or it brings new
    /// code or grants.
    Ask,
    /// Refused, and the model is told so.
    Never,
}

impl Tier {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }

    const fn max(self, other: Self) -> Self {
        if (self as u8) < (other as u8) {
            other
        } else {
            self
        }
    }
}

/// Whether calling an action only reads the system or changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Read,
    Change,
}

impl Action {
    pub const fn effect(&self) -> Effect {
        if self.facts.read_only {
            Effect::Read
        } else {
            Effect::Change
        }
    }

    /// `name(arg: type, ...) -> type`, as the script prompt shows it.
    pub const fn signature(&self) -> Signature<'_> {
        Signature(self)
    }
}

/// The tier a row runs at, from its facts and inverse, raised to `min_tier`.
pub const fn derive(row: &Action) -> Tier {
    let derived = if row.facts.read_only {
        if row.facts.open_world {
            // A fetch runs only from a grounded host (D3); none is grounded yet.
            Tier::Ask
        } else {
            Tier::Run
        }
    } else {
        match row.inverse {
            Inverse::Read | Inverse::None(_) | Inverse::Hold { .. } => Tier::Ask,
            _ if row.facts.open_world && has_payload(row.args) => Tier::Ask,
            Inverse::Restore | Inverse::Compensate(_) | Inverse::Snapshot | Inverse::Trash => {
                Tier::Run
            }
        }
    };
    derived.max(row.min_tier)
}

const fn has_payload(args: &[Arg]) -> bool {
    let mut i = 0;
    while i < args.len() {
        if matches!(args[i].role, Role::Payload) {
            return true;
        }
        i += 1;
    }
    false
}

/// Why `rows` would be refused, or `Ok`. Run at compile time over [`ROWS`].
pub const fn check(rows: &[Action]) -> Result<(), &'static str> {
    let mut i = 0;
    while i < rows.len() {
        let row = &rows[i];
        if let Err(why) = check_row(row) {
            return Err(why);
        }
        let mut j = 0;
        while j < i {
            if eq(rows[j].name, row.name) {
                return Err("two rows have one name");
            }
            j += 1;
        }
        i += 1;
    }
    Ok(())
}

const fn check_row(row: &Action) -> Result<(), &'static str> {
    if !is_name(row.name) {
        return Err("a name must be snake_case, start with a letter, at most 24 bytes");
    }
    if row.doc.is_empty() || row.doc.len() > MAX_DOC {
        return Err("a doc must be 1 to MAX_DOC bytes");
    }
    let f = row.facts;
    if f.read_only && !matches!(row.inverse, Inverse::Read) {
        return Err("a read-only row must have inverse Read");
    }
    if !f.read_only && matches!(row.inverse, Inverse::Read) {
        return Err("a change must declare how it is undone, or None");
    }
    if !f.destructive && matches!(row.inverse, Inverse::None(_)) {
        return Err("a row with no undo must be marked destructive");
    }
    if matches!(row.returns, Returns::Content(_)) && !holds_data(&row.ret) {
        return Err("a Content row must return Data");
    }
    if let Some(old) = row.old_name
        && !is_name(old)
    {
        return Err("an old name must be a valid name");
    }
    let mut a = 0;
    while a < row.args.len() {
        let arg = &row.args[a];
        if !is_name(arg.name) {
            return Err("an argument name must be snake_case");
        }
        if !f.open_world && is_host(&arg.ty) {
            return Err("a closed-world row cannot take a Host");
        }
        let mut n = 0;
        while n < arg.never.len() {
            let mut k = 0;
            while k < arg.aliases.len() {
                if eq(arg.never[n], arg.aliases[k].0) || eq(arg.never[n], arg.aliases[k].1) {
                    return Err("a never value cannot also be an alias");
                }
                k += 1;
            }
            n += 1;
        }
        a += 1;
    }
    Ok(())
}

const fn is_host(ty: &ArgType) -> bool {
    match ty {
        ArgType::Host => true,
        ArgType::List(inner, _) | ArgType::Opt(inner) => is_host(inner),
        _ => false,
    }
}

const fn holds_data(ret: &RetType) -> bool {
    match ret {
        RetType::Data => true,
        RetType::List(inner) => holds_data(inner),
        RetType::Record(fields) => {
            let mut i = 0;
            while i < fields.len() {
                if holds_data(&fields[i].1) {
                    return true;
                }
                i += 1;
            }
            false
        }
        RetType::Unit | RetType::Of(_) => false,
    }
}

/// snake_case: a lowercase letter, then lowercase letters, digits and single
/// underscores, not ending in one; at most [`MAX_NAME`] bytes.
const fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NAME || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        let ok = b.is_ascii_lowercase()
            || b.is_ascii_digit()
            || (b == b'_' && bytes[i - 1] != b'_' && i + 1 < bytes.len());
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

const fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The row named `name` in `rows`, by its name or its old name.
pub fn find<'a>(rows: &'a [Action], name: &str) -> Option<&'a Action> {
    rows.iter()
        .find(|row| row.name == name || row.old_name == Some(name))
}

const _: () = assert!(check(&ROWS).is_ok(), "an action row is inconsistent");

#[cfg(test)]
mod tests;
