//! The `agent_manifest` fuzz target's body (ADR 0013 section 6; ADR 0000 P8): an
//! agent package's manifest, its `kai-app 1` text plus the `agent 1` block, read
//! through `kaiagent::Manifest::parse` and `parse_with`, as `pkg` reads one from a
//! package an attacker may have made. `fuzz_targets/agent_manifest.rs` calls
//! [`manifest`] with libFuzzer's input, and `kaiagent/tests/hostile.rs` includes
//! this file and runs it over hostile mutations of [`SAMPLES`]. It must return for
//! any input; only a bug in the code under it may panic.
//!
//! What it holds the parser to:
//! - a refusal is one sentence, never a panic;
//! - what is read is written back as a manifest that reads back as the same one,
//!   and the written form writes itself;
//! - what is read grants no more than the text names: each `actions` entry is one
//!   of the line's words, in order; an OS row it lets the agent call is in the
//!   registry it was read against and is not one an agent may never call; the net
//!   ceiling is the destination as written; and the class, memory and budgets are
//!   the line's words;
//! - a smaller registry never takes more: what reads against part of the OS's rows
//!   reads the same against all of them.

#![allow(dead_code)]

use agentabi::{Action, ROWS, Tier};
use kaiagent::{
    Budgets, Class, MAX_KV_POSITIONS, MAX_NET_MIB_PER_DAY, MAX_POOL_MIB, MAX_TOKENS_PER_DAY,
    Manifest, NetBudget, Pattern, Refusal,
};

/// Manifests that parse: the ADR's example, an interactive one with the net line
/// (and its budget), one at the limits, and one naming OS rows and groups.
pub const SAMPLES: [&str; 4] = [
    "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
     model tokens/day=50000 kv=1024 pool=64m\nactions fs_list fs_read notes_*\n\
     memory read write\nnet none\n",
    "kai-app 1\nid weather\nabi 1.0\nagent 1\nclass interactive\n\
     model -\nactions fs_list\nmemory read\n\
     net proto:tcp,ports:443,hosts:api.weather.example bytes/day=64m\n",
    "kai-app 1\nid maxed\nabi 1.0\nagent 1\nclass background\n\
     model tokens/day=2000000 kv=2048 pool=512m\nactions fs_list fs_read fs_create\n\
     memory read write\n\
     net ports:440-443,match:exact,hosts:a.example+b.example+c.example bytes/day=512m\n",
    "kai-app 1\nid tidy\nabi 1.0\nagent 1\nclass interactive\n\
     model kv=512\nactions process_stop service_* pkg_list my_tool_*\nmemory none\n\
     net none\n",
];

/// The target: the input as a manifest, against the OS's rows and against smaller
/// registries.
pub fn manifest(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let full = read(text, &ROWS);
    assert_eq!(Manifest::parse(text), full, "parse is parse_with(ROWS)");

    // Every other row, none, and every row one an agent may never call.
    let half: Vec<Action> = ROWS.iter().copied().step_by(2).collect();
    let mut never = ROWS;
    for row in &mut never {
        row.min_tier = Tier::Never;
    }
    for rows in [&half[..], &[], &never[..]] {
        if let Ok(smaller) = read(text, rows) {
            assert_eq!(
                full.as_ref(),
                Ok(&smaller),
                "{text:?} reads wider with fewer rows"
            );
        }
    }
}

/// `text` against `rows`, held to everything above.
pub fn read(text: &str, rows: &[Action]) -> Result<Manifest, Refusal> {
    let parsed = Manifest::parse_with(text, rows);
    match &parsed {
        Err(refusal) => {
            let said = refusal.to_string();
            assert!(
                !said.is_empty() && !said.contains('\n'),
                "{refusal:?}: {said:?}"
            );
        }
        Ok(manifest) => {
            round_trips(text, manifest, rows);
            grants_what_it_names(text, manifest, rows);
        }
    }
    parsed
}

/// What was read is written as the manifest that reads back as itself.
fn round_trips(text: &str, manifest: &Manifest, rows: &[Action]) {
    let written = manifest.format();
    let again = Manifest::parse_with(&written, rows);
    assert_eq!(
        again.as_ref(),
        Ok(manifest),
        "{text:?} was read as {manifest:?}, which is written as {written:?}"
    );
    assert_eq!(
        again.unwrap().format(),
        written,
        "the written form writes itself"
    );
}

/// Each line's value in `text`, by its key: the parser took the text, so every key
/// is there once, in order.
fn value<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("{text:?} was read with no {key} line"))
}

/// The manifest grants nothing the text does not name.
fn grants_what_it_names(text: &str, manifest: &Manifest, rows: &[Action]) {
    assert_eq!(manifest.id, value(text, "id"));
    assert!(!kaiagent::BUILTIN.contains(&manifest.id.as_str()));
    assert!(!kaiagent::RESERVED.contains(&manifest.id.as_str()));
    let (major, minor) = kabi::handle::ABI;
    assert!(manifest.abi.0 == major && manifest.abi.1 <= minor);

    let class = value(text, "class");
    assert_eq!(manifest.class.name(), class);
    assert_eq!(
        manifest.asks_at_install(),
        manifest.class == Class::Background || manifest.net.is_some()
    );

    // The budgets: each the line's number, or the default when the line leaves it out.
    let model = value(text, "model");
    let Budgets {
        tokens_per_day,
        kv_positions,
        pool_mib,
    } = manifest.budgets;
    for (key, got, default, max, unit) in [
        (
            "tokens/day=",
            tokens_per_day,
            Budgets::DEFAULT.tokens_per_day,
            MAX_TOKENS_PER_DAY,
            "",
        ),
        (
            "kv=",
            kv_positions,
            Budgets::DEFAULT.kv_positions,
            MAX_KV_POSITIONS,
            "",
        ),
        (
            "pool=",
            pool_mib,
            Budgets::DEFAULT.pool_mib,
            MAX_POOL_MIB,
            "m",
        ),
    ] {
        assert!(got > 0 && got <= max, "{key}{got}");
        let written = model.split(' ').find_map(|word| word.strip_prefix(key));
        match written {
            None => assert_eq!(got, default, "{key} left out of {model:?}"),
            Some(n) => assert_eq!(with_unit(got, unit), n, "{key} in {model:?}"),
        }
    }

    // Memory: only the words the line says.
    let memory = value(text, "memory");
    assert_eq!(manifest.memory.read, memory.split(' ').any(|w| w == "read"));
    assert_eq!(
        manifest.memory.write,
        memory.split(' ').any(|w| w == "write")
    );

    // Net: the destination as written, and nothing for `none`.
    let net = value(text, "net");
    let (dest, budget) = match net.split_once(' ') {
        Some((dest, budget)) => (dest, Some(budget)),
        None => (net, None),
    };
    match &manifest.net {
        None => {
            assert_eq!(dest, "none");
            assert_eq!(manifest.net_budget, NetBudget::NONE);
        }
        Some(got) => {
            assert_eq!(netdest::Dest::parse(dest).as_ref(), Ok(got), "net {net:?}");
            let mib = manifest.net_budget.mib_per_day;
            assert!(mib > 0 && mib <= MAX_NET_MIB_PER_DAY);
            match budget {
                None => assert_eq!(manifest.net_budget, NetBudget::DEFAULT),
                Some(word) => assert_eq!(manifest.net_budget.format(), word),
            }
        }
    }

    // Actions: one entry for each of the line's words, in order.
    let actions = value(text, "actions");
    let words: Vec<&str> = match actions {
        "-" => Vec::new(),
        line => line.split(' ').collect(),
    };
    let written: Vec<String> = manifest.actions.iter().map(Pattern::format).collect();
    assert_eq!(written, words, "actions {actions:?}");

    // An OS row the agent may call is not one an agent may never call, an OS row it
    // names is in the registry it was read against, and an OS group names a row of it. (A word
    // whose first word is an OS row group is the OS's, however long its tail.)
    let os = |name: &str| agentabi::is_reserved(name.split('_').next().unwrap_or(name));
    // (A group grants the rows the registry holds in it, so a row the registry lacks
    // is no grant: what is installed is what may be called.)
    for row in rows.iter().filter(|row| manifest.allows(row.name)) {
        assert_ne!(
            row.min_tier,
            Tier::Never,
            "{actions:?} lets it call {}",
            row.name
        );
    }
    for pattern in &manifest.actions {
        if let Pattern::Row(name) = pattern
            && os(name)
        {
            assert!(
                agentabi::find(rows, name).is_some(),
                "{name}: no such row here"
            );
        }
    }
    for pattern in &manifest.actions {
        if let Pattern::Group(group) = pattern
            && os(group)
        {
            assert!(
                rows.iter().any(|row| pattern.allows(row.name)),
                "{group}_* is an OS group with no row here"
            );
        }
    }
}

/// `n` with its unit, as the line writes it.
fn with_unit(n: u32, unit: &str) -> String {
    format!("{n}{unit}")
}
