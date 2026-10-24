//! Autonomy tiers for LLM-written scripts, as the founder decided on 2026-10-20
//! (docs/roadmap.md): what the user asked for runs at once when it can be undone,
//! asks when it cannot, and never runs when it is off limits. The tier comes from
//! the action registry and the functions the script names, never from the model's
//! prompt.
//!
//! [`TIERS`] is generated from `agentabi`'s rows: one per script API function and
//! per helper the strict engine adds, each at the tier `agentabi::derive` gives it.
//! A function missing from it asks. Consent from the request (`agentabi::consent`,
//! over the analyser's plan) can only raise a call's tier above its row's.

use agentabi::Action;

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

/// The tier of `function` in `table`; one not listed asks.
pub fn of(table: &[(&str, Tier)], function: &str) -> Tier {
    table
        .iter()
        .find(|(name, _)| *name == function)
        .map_or(Tier::Ask, |&(_, tier)| tier)
}
