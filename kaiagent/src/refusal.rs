//! Why a manifest is refused: one sentence in code's words, never the model's.

use alloc::string::String;
use core::fmt;

/// What was wrong with a manifest. The parser stops at the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The text is over [`crate::MAX_BYTES`].
    TooLong,
    /// The text does not end with a newline, so it may have been cut short.
    NoNewline,
    /// A control character other than the line breaks.
    Control,
    /// The first line is not `kai-app 1`.
    NotKaiApp,
    /// The line for this key is missing or out of order, or something else stands
    /// in its place.
    Expected(&'static str),
    /// A line after the last key: format 1 has none, and a route (`use`,
    /// `expose`) is an app's, not an agent's.
    Trailing(String),
    /// The `id` is not a package name by ADR 0011's rules.
    BadId(&'static str),
    /// The `id` is one of the built-in agents' names (ADR 0013 section 1).
    BuiltinName(String),
    /// The `id` is a word other files give a meaning of their own, which no agent
    /// may be named (`all`).
    ReservedName(String),
    /// The `abi` is not `<major>.<minor>`.
    BadAbi,
    /// The `abi` is of another major, or a newer minor: this OS does not run it.
    AbiUnsupported,
    /// The agent block's version is not `agent 1`.
    AgentVersion(String),
    /// The class is neither `background` nor `interactive`.
    BadClass(String),
    /// A budget key that is unknown, repeated or out of order.
    BudgetKey(String),
    /// A budget's value is not a whole number without a sign or leading zero.
    BudgetNumber(&'static str),
    /// A budget of zero: the agent could never run.
    BudgetZero(&'static str),
    /// The pool share is not in MiB, as `64m`.
    BudgetUnit,
    /// A budget over what the OS could ever give.
    BudgetOver { key: &'static str, max: u64 },
    /// An actions entry that is not a row name or `<group>_*`.
    ActionShape(String),
    /// An actions entry listed twice, alone or by its group.
    ActionTwice(String),
    /// More entries than [`crate::MAX_ACTIONS`].
    TooManyActions,
    /// An entry in an OS row group that names no row of it.
    NoSuchOsRow(String),
    /// A group of OS rows with no row in the registry.
    GroupWithoutRows(String),
    /// A row an agent may never call.
    NeverRow(String),
    /// A group whose rows include one an agent may never call.
    GroupHoldsNever(String),
    /// A memory word that is unknown, repeated or out of order.
    Memory(String),
    /// A net line that is not `none` or a destination an agent may hold: the
    /// value, and why.
    Net(String, &'static str),
    /// The word after a net line's destination is not `bytes/day=Nm`.
    NetBudgetKey(String),
    /// The net budget is not in MiB, as `bytes/day=10m`.
    NetBudgetUnit,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(f, "a manifest is over {} bytes", crate::MAX_BYTES),
            Self::NoNewline => f.write_str("a manifest ends without a newline"),
            Self::Control => f.write_str("a manifest holds a control character"),
            Self::NotKaiApp => f.write_str("not a kai-app 1 manifest"),
            Self::Expected(key) => write!(f, "expected the {key} line, in its place"),
            Self::Trailing(line) => write!(
                f,
                "a line after the net line, which an agent has none of (routes are an app's): {line:?}"
            ),
            Self::BadId(why) => write!(f, "the id: {why}"),
            Self::BuiltinName(id) => write!(f, "{id} is a built-in agent's name"),
            Self::ReservedName(id) => write!(f, "{id} is a reserved word, not an agent's name"),
            Self::BadAbi => f.write_str("the abi is not <major>.<minor>"),
            Self::AbiUnsupported => {
                f.write_str("the abi is of another major or a newer minor than this OS runs")
            }
            Self::AgentVersion(v) => write!(f, "not agent 1: {v:?}"),
            Self::BadClass(c) => write!(f, "the class is background or interactive, not {c:?}"),
            Self::BudgetKey(k) => write!(
                f,
                "the model line takes tokens/day, kv and pool once each, in that order, not {k:?}"
            ),
            Self::BudgetNumber(key) => write!(f, "{key} is not a whole number"),
            Self::BudgetZero(key) => write!(f, "{key} is zero"),
            Self::BudgetUnit => f.write_str("pool is in MiB, as pool=64m"),
            Self::BudgetOver { key, max } => write!(f, "{key} is over {max}"),
            Self::ActionShape(e) => write!(f, "{e:?} is not a row name or <group>_*"),
            Self::ActionTwice(e) => write!(f, "{e:?} is listed twice, alone or by its group"),
            Self::TooManyActions => write!(f, "over {} actions", crate::MAX_ACTIONS),
            Self::NoSuchOsRow(e) => write!(f, "{e}: no such OS row"),
            Self::GroupWithoutRows(g) => write!(f, "{g}_*: no OS row in that group"),
            Self::NeverRow(e) => write!(f, "{e}: an agent may never call it"),
            Self::GroupHoldsNever(g) => {
                write!(f, "{g}_*: the group holds a row an agent may never call")
            }
            Self::Memory(w) => write!(f, "memory is none, read, write or read write, not {w:?}"),
            Self::Net(n, why) => write!(f, "net {n:?}: {why}"),
            Self::NetBudgetKey(w) => write!(
                f,
                "after a destination the net line takes bytes/day=Nm and nothing else, not {w:?}"
            ),
            Self::NetBudgetUnit => f.write_str("the net budget is in MiB, as bytes/day=10m"),
        }
    }
}
