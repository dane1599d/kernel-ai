//! `cargo xtask release notes [REV]` (P6 G64): a Markdown draft of the release
//! notes, from the files the signed release is made of, so that the numbers
//! the rc PR copies are never typed by hand. It is a draft for the PR to edit,
//! not the release's RELEASE-NOTES.md (release.yml writes that one).
//!
//! It prints, for REV (HEAD by default):
//! - `release/seq`;
//! - the default model's pin from `xtask/models.json`: repository, file,
//!   revision, SHA-256, size and licence;
//! - the SHA-256 of `NOTICES.txt` at REV;
//! - the image's size, as the green Release run at REV printed it
//!   (`xtask: size N bytes`, once per build; builds that differ are listed);
//! - `release check`'s lines (G62) with the run ids and files that decide
//!   each.
//!
//! A part it cannot find is written as `MISSING: why`, never left out or
//! guessed. It starts, dispatches and signs nothing; the exit is 0 whenever a
//! draft was written.

use std::process::{Command, Stdio};

use crate::release_check::{self, Facts, State, Which};
use crate::tools::sha256_hex;
use crate::workspace_root;

const USAGE: &str = "usage: cargo xtask release notes [REV]";

/// Files at a commit, as bytes.
pub trait Files {
    fn file(&self, rev: &str, path: &str) -> Option<Vec<u8>>;
}

pub fn main(args: &[String]) -> Result<(), String> {
    let rev = match args {
        [] => "HEAD",
        [rev] => rev.as_str(),
        _ => return Err(USAGE.into()),
    };
    print!("{}", draft(&release_check::Live, &Live, rev)?);
    Ok(())
}

/// The draft for `rev`.
pub fn draft(facts: &dyn Facts, files: &dyn Files, rev: &str) -> Result<String, String> {
    let sha = facts.sha(rev)?;
    let short = &sha[..sha.len().min(12)];
    let mut out = format!("# Release notes draft ({short})\n\n");
    out.push_str(&format!("- release/seq: {}\n\n", seq(facts, &sha)));
    out.push_str("## Model\n\n");
    out.push_str(&model(files, &sha));
    out.push_str("\n## Files\n\n");
    out.push_str(&format!("- NOTICES.txt: {}\n", notices(files, &sha)));
    out.push_str(&format!("- image: {}\n", image(facts, &sha, short)));
    out.push_str("\n## Checklist\n\n");
    match release_check::check(facts, &sha) {
        Ok(lines) => {
            for line in lines {
                let state = match line.state {
                    State::Met => "met",
                    State::Missing => "MISSING",
                    State::Founders => "founder's",
                };
                out.push_str(&format!("- {state}: {}: {}\n", line.what, line.detail));
            }
        }
        Err(why) => out.push_str(&format!(
            "- MISSING: the checklist could not be read: {why}\n"
        )),
    }
    Ok(out)
}

fn seq(facts: &dyn Facts, sha: &str) -> String {
    match facts.seq(sha) {
        Some(seq) => format!("`{}`", seq.trim()),
        None => "MISSING: no release/seq".into(),
    }
}

fn model(files: &dyn Files, sha: &str) -> String {
    let missing = |why: &str| format!("- MISSING: {why}\n");
    let Some(text) = files.file(sha, "xtask/models.json") else {
        return missing("no xtask/models.json");
    };
    let Ok(catalog) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return missing("xtask/models.json is not JSON");
    };
    let Some(id) = catalog["default"].as_str() else {
        return missing("models.json names no default model");
    };
    let Some(pin) = catalog["models"]
        .as_array()
        .and_then(|m| m.iter().find(|m| m["id"] == id))
    else {
        return missing(&format!("the default model `{id}` is not in the catalog"));
    };
    let field = |key: &str| match &pin[key] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "MISSING".into(),
        other => other.to_string(),
    };
    format!(
        "- `{id}`: `{}`, file `{}`, revision `{}`\n- SHA-256 `{}`, {} bytes, licence {}\n",
        field("repo"),
        field("file"),
        field("revision"),
        field("sha256"),
        field("size"),
        field("license"),
    )
}

fn notices(files: &dyn Files, sha: &str) -> String {
    match files.file(sha, "NOTICES.txt") {
        Some(bytes) => format!("SHA-256 `{}`, {} bytes", sha256_hex(&bytes), bytes.len()),
        None => "MISSING: no NOTICES.txt".into(),
    }
}

/// The sizes `xtask: size N bytes` lines of the green Release run at `sha`.
fn image(facts: &dyn Facts, sha: &str, short: &str) -> String {
    let runs = match facts.runs("release.yml", Which::At(sha)) {
        Ok(runs) => runs,
        Err(why) => return format!("MISSING: the Release runs could not be read: {why}"),
    };
    // A run from a push or a dispatch built REV's own tree; a pull_request
    // run built REV merged into its base, so it is taken last, and named.
    let at = release_check::own_tree_first(runs.into_iter().filter(|r| r.sha == sha).collect());
    let Some(run) = at.iter().find(|r| r.conclusion == "success") else {
        return format!("MISSING: no green Release run at {short}");
    };
    let log = match facts.log(run.id) {
        Ok(log) => log,
        Err(why) => return format!("MISSING: Release run {}'s log: {why}", run.named()),
    };
    let sizes = sizes(&log);
    match sizes.as_slice() {
        [] => format!("MISSING: Release run {} printed no image size", run.named()),
        [(bytes, n)] => format!(
            "{bytes} bytes ({} MiB), Release run {}, {n} builds",
            bytes >> 20,
            run.named()
        ),
        many => format!(
            "builds DIFFER, Release run {}: {}",
            run.named(),
            many.iter()
                .map(|(b, n)| format!("{b} bytes x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Each distinct `xtask: size N bytes` value in a log, and how many times it
/// appears, in order of first appearance.
pub fn sizes(log: &str) -> Vec<(u64, usize)> {
    let mut out: Vec<(u64, usize)> = Vec::new();
    for line in log.lines() {
        let Some((_, rest)) = line.split_once("xtask: size ") else {
            continue;
        };
        let Some(bytes) = rest
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        match out.iter_mut().find(|(b, _)| *b == bytes) {
            Some((_, n)) => *n += 1,
            None => out.push((bytes, 1)),
        }
    }
    out
}

struct Live;

impl Files for Live {
    fn file(&self, rev: &str, path: &str) -> Option<Vec<u8>> {
        let out = Command::new("git")
            .args(["show", &format!("{rev}:{path}")])
            .current_dir(workspace_root())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    }
}

#[cfg(test)]
mod tests;
