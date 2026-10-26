//! `cargo xtask tune record --local ID [--store DIR] [--mix A] [--shift N]`: the
//! decision the M10 exit test measures, recorded with checked provenance.
//!
//! One observe boot under icount on the held-out seed 0 runs the shell's `tune
//! editor`; the llm service traces the request it got (it has no model there).
//! The host engine then answers that very request on the pinned model, as the
//! guest would, and `xtask/tune/decisions.json` keeps: the mix and seed, the
//! summary and its SHA-256, the names it may label, the model id and the file's
//! SHA-256, the prompt hash, the raw output and the decision it parses to.
//!
//! `tune ab` checks it before measuring ([`verified_spec`]): the model must be
//! [`EXIT_MODEL`] with the catalog's pinned hash; the raw output must parse to the
//! same decision; and a new observe boot, with the recording on the disk as an
//! `llmreplay` file, must produce the same summary, so its prompt hash finds the
//! recorded answer and the shell proposes exactly that decision.

use super::abrun::icount_flags;
use super::boot::{Booter, split};
use crate::disk::ModelDisk;
use crate::esp::Module;
use llmproto::chat::Completion;
use llmproto::tune::{self, Decision, Summary, TuneRequest, replay};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `cargo xtask tune observe [--mix A] [--boots N] [--shift N]`: the observe
/// boot `boots` times (2 by default), requiring the same summary every time, so
/// a recording made on one runner checks on another.
pub fn same_main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(args, &["--mix", "--boots", "--shift"]);
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let mix = value("--mix").unwrap_or_else(|| String::from("A"));
    let boots: usize = value("--boots").map_or(Ok(2), |s| s.parse().map_err(|_| SAME_USAGE))?;
    let shift: u32 = value("--shift").map_or(Ok(crate::icount::DEFAULT_SHIFT), |s| {
        s.parse().map_err(|_| SAME_USAGE)
    })?;
    if boots < 2 {
        return Err(SAME_USAGE.into());
    }
    let booter = Booter::build(&icount_flags(rest))?;
    let mut shas = Vec::new();
    let mut texts = Vec::new();
    for boot in 1..=boots {
        let (request, _) = observe(&booter, &mix, shift, &[])?;
        let sha = sha256(request.text.as_bytes());
        println!("xtask: observe boot {boot} of {boots}: summary SHA-256 {sha}");
        shas.push(sha);
        texts.push(request.text);
    }
    if let Some(boot) = shas.iter().position(|sha| *sha != shas[0]) {
        // The lines that differ say which number moved, and so what to look at.
        for (first, other) in texts[0].lines().zip(texts[boot].lines()) {
            if first != other {
                println!("xtask: boot 1 summary:      {first}");
                println!("xtask: boot {} summary:      {other}", boot + 1);
            }
        }
        return Err(format!(
            "FAIL: the observe boots built different summaries: {}",
            shas.join(", ")
        ));
    }
    // Printed so two runners' logs show which line differs when their
    // summaries do (CI's observe-compare job sees only the hashes).
    for line in texts[0].lines() {
        println!("xtask: summary: {line}");
    }
    println!(
        "xtask: PASS: {boots} observe boots of mix {mix} built the same summary {}",
        shas[0]
    );
    Ok(())
}

const SAME_USAGE: &str = "usage: cargo xtask tune observe [--mix A] [--boots N] [--shift N]";

/// The model whose recording declares M10 done.
pub const EXIT_MODEL: &str = "qwen3-4b-2507";
/// The held-out seed the observe boot runs.
pub const OBSERVE_SEED: u64 = 0;
const USAGE: &str = "usage: cargo xtask tune record --local ID [--store DIR] [--mix A] [--shift N]";

pub fn default_path() -> PathBuf {
    crate::workspace_root().join("xtask/tune/decisions.json")
}

fn sha256(text: &[u8]) -> String {
    crate::sha256_hex(text)
}

/// Bytes of every observe boot's replay disk: the file, padded with empty lines
/// (which a replay skips), so a recording and its check boot the same command
/// line, memory and disk.
const OBSERVE_DISK_BYTES: usize = 4096;

/// The observe boot, with a replay disk holding `entries` (none when recording);
/// the request llm got.
fn observe(
    booter: &Booter,
    mix: &str,
    shift: u32,
    entries: &[(u64, Completion)],
) -> Result<(TuneRequest, Vec<String>), String> {
    let mut file = replay::encode(entries).into_bytes();
    if file.len() > OBSERVE_DISK_BYTES {
        return Err(format!(
            "the replay file is {} bytes, more than the observe disk's {OBSERVE_DISK_BYTES}",
            file.len()
        ));
    }
    file.resize(OBSERVE_DISK_BYTES, b'\n');
    let path = booter.target().join("tune-ab").join("replay.bin");
    std::fs::create_dir_all(path.parent().unwrap_or(booter.target())).map_err(|e| e.to_string())?;
    std::fs::write(&path, &file).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let disk = ModelDisk::open(&path)?;
    let cmdline = format!(
        "test=tune-ab phase=observe workload={mix} seed={OBSERVE_SEED} {} {}",
        crate::icount::CLOCK,
        disk.cmdline()
    );
    // A boot module, not a disk: Limine reads it before the kernel starts, where
    // the host-timed reads change only the absolute virtual time (which
    // `clock=icount` ignores). A disk's reads complete at host-timed moments while
    // the kernel runs, and left its state, and the summary, different per boot.
    let module = Module {
        path: String::from(crate::disk::MODEL_MODULE),
        bytes: file,
    };
    let firmware = booter.prepare_with(&cmdline, vec![module])?;
    let args = crate::icount::measure_args_with(&firmware, shift, disk.memory_mib());
    println!("xtask: observe boot: {cmdline}");
    let captured =
        crate::icount::run_capture(&booter.qemu, &args, booter.target(), booter.options.timeout)?;
    if !captured.passed {
        return Err("FAIL: the observe boot did not pass".into());
    }
    let lines = captured.lines;
    let traced = replay::untrace(lines.iter().map(String::as_str), "request");
    let (_, wire) = traced
        .first()
        .ok_or("the observe boot traced no tune request")?;
    let request = TuneRequest::decode(wire).ok_or("the traced request is not a tune request")?;
    Ok((request, lines))
}

/// What `decide` needs of the summary: its text, its target and its names.
fn summary_of(request: &TuneRequest, target: &str) -> Summary {
    Summary {
        text: request.text.clone(),
        target: 0,
        shown: Vec::new(),
        labelable: request.labelable.clone(),
        target_name: String::from(target),
        omitted: 0,
    }
}

/// The decision as `tune_spec=` reads it: `classes/firm:build-1,build-2`.
fn spec_text(decision: &Decision) -> Option<String> {
    match decision {
        Decision::Try {
            spec, background, ..
        } => Some(format!("{}:{}", spec.label(), background.join(","))),
        Decision::Keep { .. } => None,
    }
}

fn decide(request: &TuneRequest, raw: &str, cut_off: bool) -> Decision {
    let completion = Completion {
        text: raw.to_string(),
        cut_off,
    };
    tune::decide(
        &completion,
        &summary_of(request, "editor"),
        sched::spec::PolicySpec::DEFAULT,
        &[],
    )
}

pub fn main(args: &[String]) -> Result<(), String> {
    let (own, rest) = split(args, &["--local", "--store", "--mix", "--shift"]);
    let value = |name: &str| own.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let id = value("--local").ok_or(USAGE)?;
    let mix = value("--mix").unwrap_or_else(|| String::from("A"));
    let shift: u32 = value("--shift").map_or(Ok(crate::icount::DEFAULT_SHIFT), |s| {
        s.parse().map_err(|_| USAGE)
    })?;
    let model = crate::models::ModelArgs {
        local: Some(id.clone()),
        store: value("--store"),
        ..Default::default()
    };
    let loaded = crate::models::Loaded::open(&model)?;
    let bytes = loaded.into_bytes();
    let model_sha = sha256(&bytes);
    if model_sha != crate::models::pinned_sha256(&id)? {
        return Err(format!("{id}'s file does not have the catalog's SHA-256"));
    }
    let booter = Booter::build(&icount_flags(rest))?;
    let (request, _) = observe(&booter, &mix, shift, &[])?;
    let completion = super::parity::host_answer(&bytes, &request)?;
    let decision = decide(&request, &completion.text, completion.cut_off);
    let record = json!({
        "mix": mix,
        "seed": OBSERVE_SEED,
        "shift": shift,
        "summary": request.text,
        "summary_sha256": sha256(request.text.as_bytes()),
        "labelable": request.labelable,
        "target": "editor",
        "model": id,
        "model_sha256": model_sha,
        "prompt_hash": format!("{:016x}", replay::prompt_hash(&request)),
        "raw_output": completion.text,
        "cut_off": completion.cut_off,
        "decision": spec_text(&decision).unwrap_or_else(|| String::from("keep")),
    });
    let path = default_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&record).unwrap_or_default() + "\n";
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!(
        "xtask: recorded {} for {mix} at {}",
        record["decision"],
        path.display()
    );
    if id != EXIT_MODEL {
        println!("xtask: a {id} recording brings the harness up; M10 is declared on {EXIT_MODEL}");
    }
    Ok(())
}

/// Checks `decisions.json` and returns the spec `tune ab` measures.
pub fn verified_spec(booter: &Booter, path: &Path, shift: u32) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read {} ({e}): record a decision with cargo xtask tune record --local {EXIT_MODEL} (or tune-model.yml mode record), or pass --spec",
            path.display()
        )
    })?;
    let record: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let field = |name: &str| record[name].as_str().unwrap_or("").to_string();
    let model = field("model");
    if model != EXIT_MODEL {
        return Err(format!(
            "the recording is from {model}; the exit test needs {EXIT_MODEL}"
        ));
    }
    if field("model_sha256") != crate::models::pinned_sha256(&model)? {
        return Err("the recording's model hash is not the catalog's".into());
    }
    let recorded = TuneRequest {
        labelable: record["labelable"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        text: field("summary"),
    };
    if sha256(recorded.text.as_bytes()) != field("summary_sha256")
        || format!("{:016x}", replay::prompt_hash(&recorded)) != field("prompt_hash")
    {
        return Err("the recording's summary does not match its own hashes".into());
    }
    let raw = field("raw_output");
    let cut_off = record["cut_off"].as_bool().unwrap_or(true);
    let spec = spec_text(&decide(&recorded, &raw, cut_off))
        .ok_or("the recorded answer parses to keep: there is nothing to measure")?;
    if spec != field("decision") {
        return Err(format!(
            "the raw output parses to {spec}, not the recorded {}",
            field("decision")
        ));
    }
    // The guest must still build the very summary the model answered.
    let entry = (
        replay::prompt_hash(&recorded),
        Completion { text: raw, cut_off },
    );
    let mix = field("mix");
    let (now, lines) = observe(booter, &mix, shift, &[entry])?;
    if now != recorded {
        return Err(format!(
            "the guest's summary changed since the recording (SHA-256 {} now, {} recorded); re-record with cargo xtask tune record --local {EXIT_MODEL}",
            sha256(now.text.as_bytes()),
            field("summary_sha256")
        ));
    }
    let label = spec.split(':').next().unwrap_or("");
    let proposed = lines
        .iter()
        .any(|l| l.contains("tune-ab: card") && l.contains(&format!("Plan: try {label}")));
    if !proposed {
        return Err("the shell did not propose the recorded decision from the replay".into());
    }
    println!(
        "xtask: the recording's provenance holds: {model}, summary {}, decision {spec}",
        field("summary_sha256")
    );
    Ok(spec)
}
