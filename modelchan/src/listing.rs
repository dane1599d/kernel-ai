//! `kai-models 1`: what `updated` answers `models` with, the model manager's
//! list (P4 task 10). The shell and the app read it back into a
//! [`comp_models::Models`]: the signed role, the stored blobs, the model this
//! boot runs, the one the next restart uses, and the last trial. One line each,
//! a name last since it may hold spaces:
//!
//! ```text
//! kai-models 1
//! signed <64 hex> <length> <licence> <id> <name>
//! stored <64 hex> <length>
//! running <64 hex>
//! next <64 hex>
//! trial <64 hex> on-trial
//! trial <64 hex> rolled-back <why>
//! ```
//!
//! `running`, `next` and `trial` are each there at most once. A licence is an
//! SPDX expression, which may hold spaces and never `_`, so its spaces are
//! written as `_`. Text from the signed role is data, never instructions.

use crate::Sha256;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Models, Signed, Store, Stored, hex};

pub const MAGIC: &str = "kai-models 1";

/// `models` as `kai-models 1`; the disks and the import are not in it.
pub fn encode(models: &Models) -> String {
    let mut out = format!("{MAGIC}\n");
    for s in &models.signed {
        out.push_str(&format!(
            "signed {} {} {} {} {}\n",
            hex(&s.sha256),
            s.length,
            s.licence.replace(' ', "_"),
            s.id,
            s.name
        ));
    }
    for b in &models.store.blobs {
        out.push_str(&format!("stored {} {}\n", hex(&b.sha256), b.length));
    }
    let store = &models.store;
    for (key, sha) in [("running", store.running), ("next", store.next)] {
        if let Some(sha) = sha {
            out.push_str(&format!("{key} {}\n", hex(&sha)));
        }
    }
    match (&store.rolled_back, store.on_trial, store.running) {
        (Some((sha, why)), ..) => out.push_str(&format!("trial {} rolled-back {why}\n", hex(sha))),
        (None, true, Some(sha)) => out.push_str(&format!("trial {} on-trial\n", hex(&sha))),
        _ => {}
    }
    out
}

/// A list as [`encode`] writes it, parsed strictly.
pub fn parse(text: &str) -> Result<Models, String> {
    let mut lines = text
        .strip_suffix('\n')
        .ok_or("a list ends without a newline")?
        .split('\n');
    if lines.next() != Some(MAGIC) {
        return Err(format!("not {MAGIC}"));
    }
    let mut models = Models::default();
    let mut seen = Vec::new();
    for line in lines {
        let (key, rest) = line.split_once(' ').ok_or("a line with no fields")?;
        if key != "signed" && key != "stored" {
            if seen.contains(&key) {
                return Err(format!("two {key} lines"));
            }
            seen.push(key);
        }
        match key {
            "signed" => models.signed.push(signed(rest)?),
            "stored" => {
                let (sha, length) = rest.split_once(' ').ok_or("stored without a length")?;
                models.store.blobs.push(Stored {
                    sha256: sha256(sha)?,
                    length: number(length)?,
                });
            }
            "running" => models.store.running = Some(sha256(rest)?),
            "next" => models.store.next = Some(sha256(rest)?),
            "trial" => trial(rest, &mut models.store)?,
            other => return Err(format!("no line {other}")),
        }
    }
    Ok(models)
}

fn signed(rest: &str) -> Result<Signed, String> {
    let mut words = rest.splitn(5, ' ');
    let mut word = |what: &str| words.next().ok_or_else(|| format!("signed without {what}"));
    let sha = sha256(word("a hash")?)?;
    let length = number(word("a length")?)?;
    let licence = word("a licence")?.replace('_', " ");
    let id = String::from(word("an id")?);
    let name = String::from(word("a name")?);
    if [&licence, &id].iter().any(|w| w.is_empty()) || name.trim().is_empty() {
        return Err("signed with an empty field".into());
    }
    Ok(Signed {
        id,
        name,
        length,
        sha256: sha,
        licence,
        source: String::new(),
    })
}

fn trial(rest: &str, store: &mut Store) -> Result<(), String> {
    let (sha, state) = rest.split_once(' ').ok_or("trial without a state")?;
    let sha = sha256(sha)?;
    match state.split_once(' ') {
        None if state == "on-trial" => store.on_trial = true,
        Some(("rolled-back", why)) if !why.trim().is_empty() => {
            store.rolled_back = Some((sha, why.into()));
        }
        _ => return Err(format!("no trial state {state}")),
    }
    Ok(())
}

fn number(text: &str) -> Result<u64, String> {
    match text.bytes().all(|b| b.is_ascii_digit()) && !text.is_empty() && text.len() <= 20 {
        true => text.parse().map_err(|_| format!("{text} is not a length")),
        false => Err(format!("{text} is not a length")),
    }
}

/// 64 lower-case hex digits.
pub fn sha256(text: &str) -> Result<Sha256, String> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    let mut out = [0u8; 32];
    if bytes.len() != 64 {
        return Err(format!("{text} is not a SHA-256"));
    }
    for (i, pair) in bytes.chunks(2).enumerate() {
        out[i] = digit(pair[0])
            .zip(digit(pair[1]))
            .map(|(h, l)| h << 4 | l)
            .ok_or_else(|| format!("{text} is not a SHA-256"))?;
    }
    Ok(out)
}

/// What `llm`'s `health` reply says it loaded: the length and FNV-1a64 in its
/// `model <length> bytes fnv1a64 <16 hex>` words (`llm`'s `Model::describe`),
/// which the trial is judged by. None for a reply without them.
pub fn loaded(health: &str) -> Option<(u64, u64)> {
    let rest = health.split("model ").nth(1)?;
    let mut words = rest.split([' ', ';']);
    let len = words.next()?.parse().ok()?;
    let (bytes, fnv) = (words.next()?, words.next()?);
    if bytes != "bytes" || fnv != "fnv1a64" {
        return None;
    }
    let hex = words.next()?;
    (!hex.is_empty() && hex.len() <= 16)
        .then(|| u64::from_str_radix(hex, 16).ok())
        .flatten()
        .map(|fnv| (len, fnv))
}
