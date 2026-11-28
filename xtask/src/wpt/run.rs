//! Running reftests as wptrunner does (`executors/base.py`, `run_test`).
//!
//! A test's references are alternatives, tried in order, depth first. Each
//! comparison is the from-node's screenshot against the reference's, judged
//! under the from-node's fuzzy allowance for that pair. A comparison that
//! passes moves on to the reference's own references, if it is a reftest
//! item itself (a chain). The test passes as soon as one path reaches a
//! reference with none of its own. A screenshot that cannot be made is
//! `ERROR`.

use super::compare::{Difference, Picture, differences, passes};
use super::manifest::{MAX_VIEWPORT_SIDE, Manifest, Reftest, Relation};
use super::render::{Page, Renderer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// WPT's default viewport for reftests.
pub const VIEWPORT: (u32, u32) = (800, 600);
/// Chains longer than this are a cycle, or a manifest gone wrong.
pub const MAX_CHAIN: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Error(String),
}

impl Status {
    pub fn word(&self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Error(_) => "ERROR",
        }
    }
}

/// One test's result, with the last comparison made when it failed.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub url: String,
    pub path: String,
    pub status: Status,
    pub last: Option<(String, Relation, Difference)>,
    /// What the run could not honour, such as an unread `viewport_size`.
    pub note: Option<String>,
}

/// The file under `root` that `url` names: the url less its base, query and
/// fragment.
pub fn file_of(root: &Path, url_base: &str, url: &str) -> PathBuf {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path
        .strip_prefix(url_base)
        .unwrap_or(path)
        .trim_start_matches('/');
    root.join(path)
}

/// One test, through `renderer`.
pub fn run_one(
    manifest: &Manifest,
    root: &Path,
    renderer: &mut dyn Renderer,
    test: &Reftest,
) -> Outcome {
    let (width, height) = test.viewport.unwrap_or(VIEWPORT);
    let mut shots: HashMap<String, Picture> = HashMap::new();
    let mut shoot = |url: &str| -> Result<Picture, String> {
        if let Some(p) = shots.get(url) {
            return Ok(p.clone());
        }
        let file = file_of(root, &manifest.url_base, url);
        let picture = renderer.render(&Page {
            file: &file,
            url,
            width,
            height,
        })?;
        shots.insert(url.to_string(), picture.clone());
        Ok(picture)
    };
    let note = test.viewport_unread.as_ref().map(|v| {
        format!(
            "viewport_size {v:?} is not WxH with sides 1 to {}: run at {}x{}",
            MAX_VIEWPORT_SIDE, VIEWPORT.0, VIEWPORT.1
        )
    });
    let outcome = |status, last| Outcome {
        url: test.url.clone(),
        path: test.path.clone(),
        status,
        last,
        note: note.clone(),
    };
    // (from, to, relation, depth); popped from the end, so pushed reversed.
    let mut stack: Vec<(String, String, Relation, usize)> = test
        .references
        .iter()
        .rev()
        .map(|(to, rel)| (test.url.clone(), to.clone(), *rel, 1))
        .collect();
    if stack.is_empty() {
        return outcome(Status::Error(String::from("no references")), None);
    }
    let mut last = None;
    while let Some((from, to, relation, depth)) = stack.pop() {
        if depth > MAX_CHAIN {
            return outcome(
                Status::Error(format!("a reference chain past {MAX_CHAIN}")),
                last,
            );
        }
        let a = match shoot(&from) {
            Ok(p) => p,
            Err(why) => return outcome(Status::Error(why), last),
        };
        let b = match shoot(&to) {
            Ok(p) => p,
            Err(why) => return outcome(Status::Error(why), last),
        };
        let diff = differences(&a, &b);
        let fuzzy = manifest
            .reftests
            .get(&from)
            .and_then(|t| t.fuzzy_for(&from, &to, relation));
        last = Some((to.clone(), relation, diff));
        if !passes(diff, fuzzy, relation) {
            continue;
        }
        match manifest
            .reftests
            .get(&to)
            .filter(|r| !r.references.is_empty())
        {
            Some(chained) => {
                for (next, rel) in chained.references.iter().rev() {
                    stack.push((to.clone(), next.clone(), *rel, depth + 1));
                }
            }
            None => return outcome(Status::Pass, None),
        }
    }
    outcome(Status::Fail, last)
}

/// Which tests to run: those whose path starts with one of `prefixes` (all
/// when there are none), at most `limit`, in path order.
pub fn select<'m>(
    manifest: &'m Manifest,
    prefixes: &[String],
    limit: Option<usize>,
) -> Vec<&'m Reftest> {
    let mut tests: Vec<&Reftest> = manifest
        .reftests
        .values()
        .filter(|t| prefixes.is_empty() || prefixes.iter().any(|p| t.path.starts_with(p.as_str())))
        .collect();
    tests.sort_by(|a, b| (&a.path, &a.url).cmp(&(&b.path, &b.url)));
    if let Some(n) = limit {
        tests.truncate(n);
    }
    tests
}

/// Counts for one group of results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub pass: usize,
    pub fail: usize,
    pub error: usize,
}

impl Tally {
    pub fn add(&mut self, status: &Status) {
        match status {
            Status::Pass => self.pass += 1,
            Status::Fail => self.fail += 1,
            Status::Error(_) => self.error += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.pass + self.fail + self.error
    }

    /// The pass rate in percent, to one place.
    pub fn rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            (self.pass * 1000 / self.total()) as f64 / 10.0
        }
    }
}

/// The group a test is counted in: its first two path segments
/// (`css/css-flexbox`), or one for a file at the top.
pub fn group_of(path: &str) -> String {
    let mut parts = path.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), Some(_)) => format!("{a}/{b}"),
        (Some(a), Some(_), None) => a.to_string(),
        _ => String::from("."),
    }
}
