//! An agent package's manifest (ADR 0013 section 6): the package's `kai-app 1`
//! text (ADR 0001 D9) plus an `agent` block, parsed strictly and held to ADR
//! 0011's package rules. This is the host crate before the v0.1 tag: it wires
//! into nothing, and changes no input to the model.
//!
//! ```text
//! kai-app 1
//! id notes-agent
//! abi 1.0
//! agent 1
//! class background
//! model tokens/day=50000 kv=1024 pool=64m
//! actions fs_list fs_read notes_*
//! memory read write
//! net none
//! ```
//!
//! The text is a package's own, so it is read as hostile: every line, in this
//! order, once. A missing, unknown, repeated or reordered key, a line after the
//! last, or any value out of shape refuses the whole manifest, with a sentence in
//! code's words ([`Refusal`]). A new key means a new version.
//!
//! What a manifest may ask for, and what refuses it:
//! - **Identity.** `id` is the package's name under ADR 0011's rule (2 to 12 of
//!   `a-z`, `0-9` and `-`, starting with a letter; not an OS row group; not
//!   `<x>-window`) and not one of the built-in agents' names (`kai`, `goals`,
//!   `index`, `maint`). `abi` is not of another major, nor a newer minor, than the OS runs.
//! - **Class.** `background` may run without the user watching, so it always
//!   asks once at install ([`Manifest::asks_at_install`]); `interactive` runs only
//!   while its window or panel entry is open.
//! - **Budgets.** `model` holds tokens a day, KV positions and a pool share in MiB.
//!   Each is a whole number above zero; what it leaves out is question 10's
//!   default (100,000, 2,048 and 128 MiB); the pool share is at most the pool (512
//!   MiB), the KV positions at most `llm`'s slot (2,048) and the tokens a day at most
//!   2,000,000.
//! - **Actions.** Rows or `<group>_*`, at most 32; anything else is refused at the
//!   one door, so it is refused here. A row or group of the OS's own must be in the
//!   registry and none may be one an agent may never call; a package's are checked
//!   when it is installed.
//! - **Memory.** `none`, `read`, `write` or `read write`: its own scope, never
//!   another's.
//! - **Net.** `none`, or the most its network handle may reach, in `netdest`'s
//!   text form ([`net`]): TCP to named ports and named hosts, never addresses.
//!   It is a ceiling: each host still asks once, and a manifest with any asks
//!   at install.
//!
//! This crate is `no_std` + `alloc`: `pkg` and the host tests run the same code.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod actions;
mod budgets;
pub mod net;
mod refusal;
#[cfg(test)]
mod tests;

pub use actions::Pattern;
pub use budgets::{
    Budgets, DEFAULT_KV_POSITIONS, DEFAULT_POOL_MIB, DEFAULT_TOKENS_PER_DAY, MAX_KV_POSITIONS,
    MAX_POOL_MIB, MAX_TOKENS_PER_DAY,
};
pub use refusal::Refusal;

use agentabi::Action;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The most bytes a manifest may hold: it is read as hostile.
pub const MAX_BYTES: usize = 4096;
/// The most entries on the `actions` line (the same bound as an app's effects).
pub const MAX_ACTIONS: usize = 32;
/// The built-in agents (ADR 0013 section 1): native services no package may take
/// the name of.
pub const BUILTIN: [&str; 4] = ["kai", "goals", "index", "maint"];

/// When an agent may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// It may run without the user watching.
    Background,
    /// It runs only while its window or panel entry is open.
    Interactive,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Interactive => "interactive",
        }
    }
}

/// What the agent may do with its own memory scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub read: bool,
    pub write: bool,
}

impl Memory {
    fn name(self) -> &'static str {
        match (self.read, self.write) {
            (false, false) => "none",
            (true, false) => "read",
            (false, true) => "write",
            (true, true) => "read write",
        }
    }

    fn parse(value: &str) -> Result<Self, Refusal> {
        match value {
            "none" => Ok(Self::default()),
            "read" => Ok(Self {
                read: true,
                write: false,
            }),
            "write" => Ok(Self {
                read: false,
                write: true,
            }),
            "read write" => Ok(Self {
                read: true,
                write: true,
            }),
            other => Err(Refusal::Memory(other.to_string())),
        }
    }
}

/// An agent's manifest, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The package's name, and the agent's.
    pub id: String,
    /// The oldest `kabi::ABI` it needs.
    pub abi: (u32, u32),
    pub class: Class,
    pub budgets: Budgets,
    pub actions: Vec<Pattern>,
    pub memory: Memory,
    /// The most its network handle may reach, or `None` for `net none`.
    pub net: Option<netdest::Dest>,
}

impl Manifest {
    /// Reads and checks `text` against the OS's own rows (`agentabi::ROWS`).
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        Self::parse_with(text, &agentabi::ROWS)
    }

    /// [`Manifest::parse`] against `rows`, the registry an OS row or group must be
    /// in.
    pub fn parse_with(text: &str, rows: &[Action]) -> Result<Self, Refusal> {
        if text.len() > MAX_BYTES {
            return Err(Refusal::TooLong);
        }
        if text.chars().any(|c| c.is_control() && c != '\n') {
            return Err(Refusal::Control);
        }
        let mut lines = text
            .strip_suffix('\n')
            .ok_or(Refusal::NoNewline)?
            .split('\n');
        if lines.next() != Some("kai-app 1") {
            return Err(Refusal::NotKaiApp);
        }
        let mut field = |key: &'static str| -> Result<&str, Refusal> {
            lines
                .next()
                .and_then(|line| line.strip_prefix(key))
                .and_then(|rest| rest.strip_prefix(' '))
                .filter(|value| !value.is_empty())
                .ok_or(Refusal::Expected(key))
        };
        let id = field("id")?.to_string();
        valid_id(&id)?;
        let abi = abi(field("abi")?)?;
        match field("agent")? {
            "1" => {}
            other => return Err(Refusal::AgentVersion(other.to_string())),
        }
        let class = match field("class")? {
            "background" => Class::Background,
            "interactive" => Class::Interactive,
            other => return Err(Refusal::BadClass(other.to_string())),
        };
        let budgets = Budgets::parse(field("model")?)?;
        let actions = actions::parse(field("actions")?, rows)?;
        let memory = Memory::parse(field("memory")?)?;
        let net = net::parse(field("net")?)?;
        if let Some(extra) = lines.next() {
            return Err(Refusal::Trailing(extra.to_string()));
        }
        Ok(Self {
            id,
            abi,
            class,
            budgets,
            actions,
            memory,
            net,
        })
    }

    /// The manifest as it is written, every key, in order: what [`Manifest::parse`]
    /// reads back as this one.
    pub fn format(&self) -> String {
        let actions = match self.actions.is_empty() {
            true => String::from("-"),
            false => self
                .actions
                .iter()
                .map(Pattern::format)
                .collect::<Vec<_>>()
                .join(" "),
        };
        alloc::format!(
            "kai-app 1\nid {}\nabi {}.{}\nagent 1\nclass {}\nmodel {}\nactions {}\nmemory {}\nnet {}\n",
            self.id,
            self.abi.0,
            self.abi.1,
            self.class.name(),
            self.budgets.format(),
            actions,
            self.memory.name(),
            net::format(&self.net),
        )
    }

    /// Whether installing it always asks, whatever else it asks: a `background`
    /// agent will act while the user is away, and one with a `net` line can
    /// reach out of the machine (ADR 0013 section 6). The ask lists its actions,
    /// budgets, memory use and hosts; the other installs follow ADR 0011
    /// section 10.
    pub fn asks_at_install(&self) -> bool {
        self.class == Class::Background || self.net.is_some()
    }

    /// Whether the manifest lets the agent call the row `name`. Whether it may in
    /// fact is for the tiers to say, after this.
    pub fn allows(&self, name: &str) -> bool {
        self.actions.iter().any(|pattern| pattern.allows(name))
    }
}

/// `id` as an agent's identity: a package's name (ADR 0011 section 2) that no
/// built-in agent has.
fn valid_id(id: &str) -> Result<(), Refusal> {
    kaipkg::header::valid_name(id).map_err(Refusal::BadId)?;
    match BUILTIN.contains(&id) {
        true => Err(Refusal::BuiltinName(id.to_string())),
        false => Ok(()),
    }
}

/// `<major>.<minor>`, not newer than the OS's `kabi::ABI` (ADR 0001 D9: the oldest
/// ABI the manifest needs).
fn abi(value: &str) -> Result<(u32, u32), Refusal> {
    let number = |digits: &str| {
        let plain = !digits.is_empty()
            && digits.len() <= 4
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        plain
            .then(|| digits.parse::<u32>().ok())
            .flatten()
            .ok_or(Refusal::BadAbi)
    };
    let (major, minor) = value.split_once('.').ok_or(Refusal::BadAbi)?;
    let (major, minor) = (number(major)?, number(minor)?);
    let (have_major, have_minor) = kabi::handle::ABI;
    match (major, minor) {
        (m, n) if m != have_major || n > have_minor => Err(Refusal::AbiUnsupported),
        _ => Ok((major, minor)),
    }
}
