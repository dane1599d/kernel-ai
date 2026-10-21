//! Autonomy tiers for LLM-written scripts, as the founder decided on 2026-10-20
//! (docs/roadmap.md): what the user asked for runs at once when it can be undone,
//! asks when it cannot, and never runs when it is off limits. The tier comes from
//! this table and the functions the script names, never from the model's prompt.
//!
//! The table is data so P1's action registry can take it over: one row per script
//! API function. A function missing from it asks.

use alloc::vec::Vec;

/// How a script that calls a function is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Runs at once: it only reads, or the shell records how to undo it.
    Run,
    /// Runs only after a yes: it has no undo.
    Ask,
    /// Refused, and the model is told so.
    Never,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }
}

/// Every script API function and its tier. A `Run` function that changes anything
/// must record its inverse when it runs (`script::register`); the tests check that
/// each one does.
pub const TIERS: &[(&str, Tier)] = &[
    ("cpus", Tier::Run),
    ("memory", Tier::Run),
    ("uptime_ms", Tier::Run),
    ("processes", Tier::Run),
    ("threads", Tier::Run),
    ("get_policy", Tier::Run),
    // The previous policy is recorded first, so `undo` puts it back.
    ("set_policy", Tier::Run),
];

/// The tier of `function` in `table`; one not listed asks.
pub fn of(table: &[(&str, Tier)], function: &str) -> Tier {
    table
        .iter()
        .find(|(name, _)| *name == function)
        .map_or(Tier::Ask, |&(_, tier)| tier)
}

/// The strictest tier among `functions`, and the functions at that tier.
pub fn of_script(table: &[(&str, Tier)], functions: &[&'static str]) -> (Tier, Vec<&'static str>) {
    let tier = functions
        .iter()
        .map(|name| of(table, name))
        .max()
        .unwrap_or(Tier::Run);
    let at = functions
        .iter()
        .copied()
        .filter(|name| of(table, name) == tier)
        .collect();
    (tier, at)
}
