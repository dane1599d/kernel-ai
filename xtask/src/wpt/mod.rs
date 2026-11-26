//! `cargo xtask wpt`: web-platform-tests reftests through a pluggable renderer
//! (P6 groundwork G10).
//!
//! WPT is never committed, and host tests never fetch it.
//! `.github/workflows/web-wpt.yml` (workflow_dispatch only) does three things:
//! - fetches WPT at [`COMMIT`];
//! - builds its `MANIFEST.json` with WPT's own tool;
//! - runs this command and keeps `report.md` and `results.json` as an
//!   artifact.
//!
//! The manifest parser (`manifest.rs`), the pixel comparison with WPT's fuzzy
//! rules (`compare.rs`) and the reference walk (`run.rs`) are host-tested on
//! small fixtures (`tests.rs`). Until B-1's engine lands, the only renderer
//! is [`render::Stub`], whose pass rate says nothing about rendering.
//!
//! ```text
//! cargo xtask wpt commit
//! cargo xtask wpt run --wpt DIR [--manifest PATH] [--renderer stub]
//!                     [--filter PATH-PREFIX]... [--limit N] [--out DIR]
//! ```
//!
//! - `commit` prints the pinned WPT commit, so the workflow and this file
//!   name one commit.
//! - `run` writes `report.md` (totals and pass rate by directory) and
//!   `results.json` (every test: status, message, and the last comparison's
//!   figures) to `--out`, `wpt-out` by default.

mod compare;
mod manifest;
mod render;
mod run;
#[cfg(test)]
mod tests;

use run::{Outcome, Status, Tally};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// The WPT commit the harness runs at (master on 2026-11-26).
pub const COMMIT: &str = "1d8b186e1d0c366d20e8e38c748e4b843b426674";

pub fn main(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("commit") => {
            println!("{COMMIT}");
            Ok(())
        }
        Some("run") => run_command(&args[1..]).map_err(|why| format!("wpt: {why}")),
        _ => Err(String::from(
            "usage: cargo xtask wpt commit | cargo xtask wpt run --wpt DIR [--manifest PATH] [--renderer stub] [--filter PREFIX]... [--limit N] [--out DIR]",
        )),
    }
}

struct Options {
    wpt: PathBuf,
    manifest: Option<PathBuf>,
    renderer: String,
    filters: Vec<String>,
    limit: Option<usize>,
    out: PathBuf,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        wpt: PathBuf::new(),
        manifest: None,
        renderer: String::from("stub"),
        filters: Vec::new(),
        limit: None,
        out: PathBuf::from("wpt-out"),
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--wpt" => o.wpt = PathBuf::from(value()?),
            "--manifest" => o.manifest = Some(PathBuf::from(value()?)),
            "--renderer" => o.renderer = value()?,
            "--filter" => o.filters.push(value()?.trim_start_matches('/').to_string()),
            "--limit" => o.limit = Some(value()?.parse().map_err(|_| "--limit needs a number")?),
            "--out" => o.out = PathBuf::from(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if o.wpt.as_os_str().is_empty() {
        return Err(String::from("--wpt DIR is needed: a WPT checkout"));
    }
    Ok(o)
}

fn run_command(args: &[String]) -> Result<(), String> {
    let o = options(args)?;
    let manifest_path = o
        .manifest
        .clone()
        .unwrap_or_else(|| o.wpt.join("MANIFEST.json"));
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = manifest::parse(&text)?;
    let mut renderer = render::by_name(&o.renderer).ok_or(format!("no renderer {}", o.renderer))?;
    let tests = run::select(&manifest, &o.filters, o.limit);
    eprintln!(
        "xtask wpt: {} of {} reftests, renderer {}",
        tests.len(),
        manifest.reftests.len(),
        renderer.name()
    );
    let mut outcomes = Vec::with_capacity(tests.len());
    for (n, test) in tests.iter().enumerate() {
        outcomes.push(run::run_one(&manifest, &o.wpt, renderer.as_mut(), test));
        if (n + 1) % 1000 == 0 {
            eprintln!("xtask wpt: {} done", n + 1);
        }
    }
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let report = report(&outcomes, renderer.name(), &o.filters);
    std::fs::write(o.out.join("report.md"), &report).map_err(|e| e.to_string())?;
    std::fs::write(
        o.out.join("results.json"),
        results_json(&outcomes, renderer.name()),
    )
    .map_err(|e| e.to_string())?;
    print!("{report}");
    Ok(())
}

/// The Markdown report: totals, then pass rates by directory.
pub fn report(outcomes: &[Outcome], renderer: &str, filters: &[String]) -> String {
    let mut all = Tally::default();
    let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
    for o in outcomes {
        all.add(&o.status);
        groups
            .entry(run::group_of(&o.path))
            .or_default()
            .add(&o.status);
    }
    let mut out = String::new();
    let _ = writeln!(out, "# WPT reftests, renderer `{renderer}`\n");
    let _ = writeln!(out, "WPT commit `{COMMIT}`.");
    if !filters.is_empty() {
        let _ = writeln!(out, "Paths under: {}.", filters.join(", "));
    }
    if renderer == "stub" {
        let _ = writeln!(
            out,
            "\nThe stub draws each file as one flat colour from its bytes: this pass rate says nothing about rendering."
        );
    }
    let _ = writeln!(
        out,
        "\n**{} of {} passed ({:.1}%)**; {} failed, {} errors.\n",
        all.pass,
        all.total(),
        all.rate(),
        all.fail,
        all.error
    );
    let _ = writeln!(
        out,
        "| Directory | Pass | Fail | Error | Pass rate |\n|---|---|---|---|---|"
    );
    for (group, t) in &groups {
        let _ = writeln!(
            out,
            "| {group} | {} | {} | {} | {:.1}% |",
            t.pass,
            t.fail,
            t.error,
            t.rate()
        );
    }
    out
}

/// Every result as JSON, for comparing two runs.
pub fn results_json(outcomes: &[Outcome], renderer: &str) -> String {
    let tests: Vec<serde_json::Value> = outcomes
        .iter()
        .map(|o| {
            let mut v = serde_json::json!({
                "url": o.url,
                "path": o.path,
                "status": o.status.word(),
            });
            if let Status::Error(why) = &o.status {
                v["message"] = serde_json::Value::from(why.as_str());
            }
            if let Some((reference, relation, diff)) = &o.last {
                v["last"] = serde_json::json!({
                    "reference": reference,
                    "relation": relation.symbol(),
                    "max_per_channel": diff.per_channel,
                    "pixels_different": diff.pixels,
                    "size_differs": diff.size,
                });
            }
            v
        })
        .collect();
    let doc = serde_json::json!({ "wpt_commit": COMMIT, "renderer": renderer, "tests": tests });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}
