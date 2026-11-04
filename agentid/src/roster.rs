//! The roster: which agents exist, each with its badge, its name and what it may
//! do (ADR 0013 section 1).
//!
//! - **The badge is the runtime identity.** The supervisor mints one per agent at
//!   spawn and the kernel delivers it on the route to `llm` and to the door, so an
//!   agent cannot claim another's. A roster mints them the same way: in order, never
//!   twice. The four built-in agents come first (`kai`, `goals`, `index`, `maint`) with
//!   the badges 1 to 4; a packaged agent gets the next number up when it is installed,
//!   and the number is never handed out again, even after the agent is retired.
//! - **The name is the durable identity.** Badges do not outlive a boot; the log does.
//!   A name belongs to one live agent at a time.
//! - **A packaged agent is its manifest.** [`Roster::install`] takes a checked
//!   [`kaiagent::Manifest`] and binds it to a badge; the agent's ceiling at the door
//!   and its budgets are the manifest's, and an upgrade replaces them under the same
//!   badge.
//! - **The built-in agents hold nothing but `kai`.** `kai` is the shell, which is you:
//!   any row, with the tiers deciding. `goals`, `index` and `maint` hold no row until
//!   their rows exist (A-core, after the v0.1 tag), so everything they try is refused
//!   at the door.

use crate::name::Name;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use kaiagent::{Budgets, Manifest};

/// The most packaged agents installed at once.
pub const MAX_PACKAGED: usize = 32;

/// The next roster's serial: a caller made by one roster is not live on another.
static NEXT_SERIAL: AtomicU32 = AtomicU32::new(1);

/// An agent's badge. Only a [`Roster`] makes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(u32);

impl AgentId {
    /// The number the kernel delivers: what `llm` keys its queues, quotas and KV slots
    /// on.
    pub const fn badge(self) -> u32 {
        self.0
    }
}

/// An agent that ships with Kernel AI: a native service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// The shell, which is you.
    Kai,
    /// Runs goals (section 5).
    Goals,
    /// Embeds files and memories: a model client with no actions.
    Index,
    /// Self-maintenance (section 7).
    Maint,
}

impl Builtin {
    /// All four, in badge order.
    pub const ALL: [Builtin; 4] = [Self::Kai, Self::Goals, Self::Index, Self::Maint];

    /// Its name: the one in [`kaiagent::BUILTIN`], which no package may take.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kai => "kai",
            Self::Goals => "goals",
            Self::Index => "index",
            Self::Maint => "maint",
        }
    }

    pub(crate) const fn id(self) -> AgentId {
        AgentId(self as u32 + 1)
    }
}

/// What an agent is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Builtin(Builtin),
    /// An installed package, as its manifest says.
    Packaged(Manifest),
}

/// One agent on the roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    id: AgentId,
    name: Name,
    kind: Kind,
}

impl Agent {
    pub fn id(&self) -> AgentId {
        self.id
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// The manifest of a packaged agent.
    pub fn manifest(&self) -> Option<&Manifest> {
        match &self.kind {
            Kind::Packaged(manifest) => Some(manifest),
            Kind::Builtin(_) => None,
        }
    }

    /// What `llm` holds a packaged agent to; the built-in agents' come from the
    /// scheduler's own policy, not a manifest.
    pub fn budgets(&self) -> Option<&Budgets> {
        self.manifest().map(|manifest| &manifest.budgets)
    }

    /// Whether the agent's own grants let it call the row `row`. Whether it may in
    /// fact is for the tiers to say, after this.
    pub fn allows(&self, row: &str) -> bool {
        match &self.kind {
            Kind::Builtin(Builtin::Kai) => true,
            Kind::Builtin(_) => false,
            Kind::Packaged(manifest) => manifest.allows(row),
        }
    }
}

/// Who is calling: an agent the roster knew when it was made. Only a [`Roster`] makes
/// one, from the badge the kernel delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    roster: u32,
    id: AgentId,
    name: Name,
}

impl Caller {
    pub fn id(&self) -> AgentId {
        self.id
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    /// Whether the caller is the person's own agent: by its badge, which only the
    /// roster's first agent has, not by its name.
    pub fn is_kai(&self) -> bool {
        self.id == Builtin::Kai.id()
    }

    /// Whether the caller may undo the entry that `owner` made: `kai` any entry, an
    /// agent its own.
    pub fn may_undo(&self, owner: &Name) -> bool {
        self.is_kai() || &self.name == owner
    }
}

/// Why an agent was not installed or upgraded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// A live agent has the name already.
    Taken(Name),
    /// [`MAX_PACKAGED`] packaged agents are installed.
    Full,
    /// The manifest's id is not an agent's name.
    BadName,
    /// No packaged agent has the badge.
    NotPackaged,
    /// An upgrade keeps the name.
    NameChanged,
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Taken(name) => write!(f, "{name} is already an agent"),
            Self::Full => write!(f, "{MAX_PACKAGED} agents are installed already"),
            Self::BadName => write!(f, "the manifest's id is not an agent's name"),
            Self::NotPackaged => write!(f, "no installed agent has that badge"),
            Self::NameChanged => write!(f, "an upgrade keeps the agent's name"),
        }
    }
}

/// A badge that no live agent holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownBadge(pub u32);

impl fmt::Display for UnknownBadge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no agent has badge {}", self.0)
    }
}

/// The agents that exist.
#[derive(Clone, Debug)]
pub struct Roster {
    /// Which roster this is, so its callers are its own.
    serial: u32,
    agents: Vec<Agent>,
    /// The next badge to mint; never goes back.
    next: u32,
}

impl Default for Roster {
    fn default() -> Self {
        Self::new()
    }
}

impl Roster {
    /// A roster of the four built-in agents.
    pub fn new() -> Self {
        let agents = Builtin::ALL
            .iter()
            .map(|&which| Agent {
                id: which.id(),
                name: Name::builtin(which.name()),
                kind: Kind::Builtin(which),
            })
            .collect();
        Self {
            serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
            agents,
            next: Builtin::ALL.len() as u32 + 1,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Agent> {
        self.agents.iter()
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn get(&self, id: AgentId) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.name.as_str() == name)
    }

    /// The agent behind a badge the kernel delivered, as a caller.
    pub fn caller(&self, badge: u32) -> Result<Caller, UnknownBadge> {
        self.agents
            .iter()
            .find(|agent| agent.id.badge() == badge)
            .map(|agent| Caller {
                roster: self.serial,
                id: agent.id,
                name: agent.name.clone(),
            })
            .ok_or(UnknownBadge(badge))
    }

    /// Whether `caller` is still the live agent it was made for, on this roster: its
    /// badge and name both, so a retired agent's caller does not pass for a later agent
    /// of the same name.
    pub fn is_live(&self, caller: &Caller) -> bool {
        caller.roster == self.serial
            && self
                .get(caller.id)
                .is_some_and(|agent| agent.name == caller.name)
    }

    /// Installs `manifest`'s agent under a new badge.
    pub fn install(&mut self, manifest: Manifest) -> Result<AgentId, InstallError> {
        let name = Name::new(&manifest.id).map_err(|_| InstallError::BadName)?;
        if self.by_name(name.as_str()).is_some() {
            return Err(InstallError::Taken(name));
        }
        let packaged = self
            .agents
            .iter()
            .filter(|agent| agent.manifest().is_some())
            .count();
        if packaged >= MAX_PACKAGED {
            return Err(InstallError::Full);
        }
        let id = AgentId(self.next);
        self.next = self.next.checked_add(1).ok_or(InstallError::Full)?;
        self.agents.push(Agent {
            id,
            name,
            kind: Kind::Packaged(manifest),
        });
        Ok(id)
    }

    /// Replaces a packaged agent's manifest under the same badge and name: its
    /// ceiling and budgets change, its identity does not.
    pub fn upgrade(&mut self, id: AgentId, manifest: Manifest) -> Result<(), InstallError> {
        let agent = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == id && agent.manifest().is_some())
            .ok_or(InstallError::NotPackaged)?;
        if agent.name.as_str() != manifest.id {
            return Err(InstallError::NameChanged);
        }
        agent.kind = Kind::Packaged(manifest);
        Ok(())
    }

    /// Removes a packaged agent. Its badge is never handed out again; a package of
    /// the same name installed later is a new agent. The built-in agents stay.
    pub fn retire(&mut self, id: AgentId) -> Result<Agent, InstallError> {
        let at = self
            .agents
            .iter()
            .position(|agent| agent.id == id && agent.manifest().is_some())
            .ok_or(InstallError::NotPackaged)?;
        Ok(self.agents.remove(at))
    }
}
