//! Autonomy tiers for LLM-written scripts, as the founder decided on 2026-10-20
//! (docs/roadmap.md): what the user asked for runs at once when it can be undone,
//! asks when it cannot, and never runs when it is off limits. The tier comes from
//! the action registry and the functions the script names, never from the model's
//! prompt.
//!
//! [`TIERS`] is generated from `agentabi`'s rows: one per script API function and
//! per helper the strict engine adds, each at the tier `agentabi::derive` gives it.
//! A function missing from it asks. Until task 7's consent check, a run-tier change
//! also asks when the request names none of its row's `cues`: the model may not add
//! a change.

use agentabi::{Action, Effect};
use alloc::vec::Vec;

pub use agentabi::Tier;

/// The rows scripts can call: the script API, then the helpers.
const CALLABLE: usize = agentabi::SCRIPT_API.len() + agentabi::HELPERS.len();

const fn callable(i: usize) -> &'static Action {
    let api = agentabi::SCRIPT_API.len();
    if i < api {
        &agentabi::SCRIPT_API[i]
    } else {
        &agentabi::HELPERS[i - api]
    }
}

const fn table() -> [(&'static str, Tier); CALLABLE] {
    let mut out = [("", Tier::Ask); CALLABLE];
    let mut i = 0;
    while i < CALLABLE {
        let row = callable(i);
        out[i] = (row.name, agentabi::derive(row));
        i += 1;
    }
    out
}

const TABLE: [(&str, Tier); CALLABLE] = table();

/// Every function a script can call and its tier, derived from its row. A `Run`
/// function that changes anything must record its inverse when it runs
/// (`script::register`); the tests check that each one does.
pub const TIERS: &[(&str, Tier)] = &TABLE;

/// Whether `request` names what `function` changes: one of its row's `cues`, any
/// case. A read, or a change whose row has no cues, needs none. Interim, until
/// task 7 derives consent from the whole row.
pub fn asked_for(request: &str, function: &str) -> bool {
    let request = request.to_ascii_lowercase();
    agentabi::find(&agentabi::SCRIPT_API, function)
        .filter(|row| row.effect() == Effect::Change && !row.cues.is_empty())
        .is_none_or(|row| row.cues.iter().any(|cue| request.contains(cue)))
}

/// The tier of `function` in `table`; one not listed asks.
pub fn of(table: &[(&str, Tier)], function: &str) -> Tier {
    table
        .iter()
        .find(|(name, _)| *name == function)
        .map_or(Tier::Ask, |&(_, tier)| tier)
}

/// The tier of `function` in a script written for `request`: its row's, raised to
/// `Ask` when the request did not ask for it, or is not in English.
pub fn for_request(table: &[(&str, Tier)], request: &str, function: &str) -> Tier {
    match of(table, function) {
        Tier::Run if !asked_for(request, function) => Tier::Ask,
        // v0.1 gives a line that is not English no consent for changes (ADR 0002 D3).
        Tier::Run if changes(function) && !agentabi::consent::is_english_line(request) => Tier::Ask,
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

/// Whether `function` is a script API row that changes something.
fn changes(function: &str) -> bool {
    agentabi::find(&agentabi::SCRIPT_API, function)
        .is_some_and(|row| row.effect() == Effect::Change)
}
