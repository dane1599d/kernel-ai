//! Autonomy tiers for LLM-written scripts, as the founder decided on 2026-10-20
//! (docs/roadmap.md): what the user asked for runs at once when it can be undone,
//! asks when it cannot, and never runs when it is off limits. The tier comes from
//! this table and the functions the script names, never from the model's prompt.
//!
//! The table is data so P1's action registry can take it over: one row per script
//! API function and per helper the strict engine adds. A function missing from it
//! asks. Until P1's consent check, a run-tier change also asks when the request
//! does not name what it changes (`CUES`): the model may not add a change.

use alloc::vec::Vec;

/// How a script that calls a function is handled.
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
    // Pure helpers every strict engine has (`script::HELPERS`).
    ("fixed", Tier::Run),
    ("floor", Tier::Run),
];

/// Words a request must contain, any case, for a run-tier change to run at once;
/// without one, the model added the change and it asks. A function with no row
/// here needs none. Interim, until P1 derives consent from the action registry.
pub const CUES: &[(&str, &[&str])] = &[(
    "set_policy",
    &[
        "polic",
        "schedul",
        "round robin",
        "round-robin",
        "interactive",
    ],
)];

/// Whether `request` names what `function` changes (see [`CUES`]).
pub fn asked_for(request: &str, function: &str) -> bool {
    let request = request.to_ascii_lowercase();
    CUES.iter()
        .find(|(name, _)| *name == function)
        .is_none_or(|(_, cues)| cues.iter().any(|cue| request.contains(cue)))
}

/// The tier of `function` in `table`; one not listed asks.
pub fn of(table: &[(&str, Tier)], function: &str) -> Tier {
    table
        .iter()
        .find(|(name, _)| *name == function)
        .map_or(Tier::Ask, |&(_, tier)| tier)
}

/// The tier of `function` in a script written for `request`: its row's, raised to
/// `Ask` when the request did not ask for it.
pub fn for_request(table: &[(&str, Tier)], request: &str, function: &str) -> Tier {
    match of(table, function) {
        Tier::Run if !asked_for(request, function) => Tier::Ask,
        tier => tier,
    }
}

/// The strictest tier among `functions` in a script written for `request`, and
/// the functions at that tier.
pub fn of_script<'a>(
    table: &[(&str, Tier)],
    request: &str,
    functions: &[&'a str],
) -> (Tier, Vec<&'a str>) {
    let tier_of = |name: &str| for_request(table, request, name);
    let tier = functions
        .iter()
        .map(|name| tier_of(name))
        .max()
        .unwrap_or(Tier::Run);
    let at = functions
        .iter()
        .copied()
        .filter(|name| tier_of(name) == tier)
        .collect();
    (tier, at)
}
