//! WPT's `MANIFEST.json` (tools/manifest, version 9), its `reftest` items.
//!
//! `items.reftest` is a tree of path segments. Each leaf is a file:
//! `[hash, item, item, ...]`. Each item is `[url or null, references,
//! extras]`:
//! - a null url means the file's own path under `url_base`;
//! - `references` are `[url, "==" | "!="]` pairs;
//! - `extras` may hold `fuzzy`, `viewport_size`, `timeout`, `dpi` and
//!   `testdriver`.
//!
//! `fuzzy` is a list of `[key, [[min, max], [min, max]]]`: the allowed
//! largest per-channel difference, then the allowed count of differing
//! pixels. Its key is null (any reference), a ref url, or
//! `[test url, ref url, relation]`. A reference that is itself a reftest item
//! is chained through its own references, as wptrunner does (see `run.rs`).

use serde_json::Value;
use std::collections::BTreeMap;

/// A reference's relation to its test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// `==`, `<link rel=match>`: the two must look the same.
    Match,
    /// `!=`, `<link rel=mismatch>`: the two must not.
    Mismatch,
}

impl Relation {
    pub fn symbol(self) -> &'static str {
        match self {
            Relation::Match => "==",
            Relation::Mismatch => "!=",
        }
    }
}

/// Allowed differences: the largest per-channel difference, then the count of
/// differing pixels, each an inclusive range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fuzzy {
    pub per_channel: (u32, u32),
    pub pixels: (u64, u64),
}

/// Which comparisons a [`Fuzzy`] is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FuzzyKey {
    /// Any of the test's references.
    Any,
    /// One reference, by url.
    Ref(String),
    /// One comparison: test url, ref url and relation.
    Full(String, String, Relation),
}

/// One reftest item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reftest {
    /// The file under the WPT root, `/`-separated.
    pub path: String,
    /// Its url: the path under `url_base`, perhaps with a query (variants).
    pub url: String,
    pub references: Vec<(String, Relation)>,
    pub fuzzy: Vec<(FuzzyKey, Fuzzy)>,
    /// `viewport_size`, when the test sets one: width, height.
    pub viewport: Option<(u32, u32)>,
}

/// The reftests of a manifest, by url.
#[derive(Debug, Default)]
pub struct Manifest {
    pub url_base: String,
    pub reftests: BTreeMap<String, Reftest>,
}

/// Parses a manifest's text. Only `reftest` items are kept; a malformed one
/// is an error naming its path, so a format change is seen, not skipped.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("MANIFEST.json: {e}"))?;
    let version = root.get("version").and_then(Value::as_u64);
    if version != Some(9) {
        return Err(format!(
            "MANIFEST.json: version {version:?}, but this reads version 9"
        ));
    }
    let url_base = root
        .get("url_base")
        .and_then(Value::as_str)
        .unwrap_or("/")
        .to_string();
    let mut manifest = Manifest {
        url_base,
        reftests: BTreeMap::new(),
    };
    if let Some(tree) = root.get("items").and_then(|i| i.get("reftest")) {
        walk(tree, &mut Vec::new(), &mut manifest)?;
    }
    Ok(manifest)
}

fn walk(node: &Value, segments: &mut Vec<String>, out: &mut Manifest) -> Result<(), String> {
    match node {
        Value::Object(children) => {
            for (name, child) in children {
                segments.push(name.clone());
                walk(child, segments, out)?;
                segments.pop();
            }
            Ok(())
        }
        Value::Array(leaf) => {
            let path = segments.join("/");
            // The first entry is the file's hash; each after it, an item.
            for item in leaf.iter().skip(1) {
                let test =
                    item_of(&path, &out.url_base, item).map_err(|why| format!("{path}: {why}"))?;
                out.reftests.insert(test.url.clone(), test);
            }
            Ok(())
        }
        _ => Err(format!(
            "{}: neither a directory nor a file",
            segments.join("/")
        )),
    }
}

fn item_of(path: &str, url_base: &str, item: &Value) -> Result<Reftest, String> {
    let fields = item
        .as_array()
        .filter(|f| f.len() == 3)
        .ok_or("an item is not [url, references, extras]")?;
    let url = match &fields[0] {
        Value::Null => format!("{}{path}", url_base),
        Value::String(url) => url.clone(),
        _ => return Err(String::from("a url is neither null nor text")),
    };
    let mut references = Vec::new();
    for reference in fields[1].as_array().ok_or("references are not a list")? {
        let pair = reference
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or("a reference is not [url, relation]")?;
        let url = pair[0].as_str().ok_or("a reference's url is not text")?;
        references.push((url.to_string(), relation(&pair[1])?));
    }
    let extras = fields[2].as_object().ok_or("extras are not an object")?;
    let mut fuzzy = Vec::new();
    if let Some(list) = extras.get("fuzzy") {
        for entry in list.as_array().ok_or("fuzzy is not a list")? {
            let pair = entry
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("a fuzzy entry is not [key, ranges]")?;
            fuzzy.push((fuzzy_key(&pair[0])?, ranges(&pair[1])?));
        }
    }
    let viewport = match extras.get("viewport_size") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let wh = v
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or("viewport_size is not [width, height]")?;
            let side = |v: &Value| {
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("a viewport side is not a number")
            };
            Some((side(&wh[0])?, side(&wh[1])?))
        }
    };
    Ok(Reftest {
        path: path.to_string(),
        url,
        references,
        fuzzy,
        viewport,
    })
}

fn relation(v: &Value) -> Result<Relation, String> {
    match v.as_str() {
        Some("==") => Ok(Relation::Match),
        Some("!=") => Ok(Relation::Mismatch),
        other => Err(format!("a relation {other:?}, not == or !=")),
    }
}

fn fuzzy_key(v: &Value) -> Result<FuzzyKey, String> {
    match v {
        Value::Null => Ok(FuzzyKey::Any),
        Value::String(url) => Ok(FuzzyKey::Ref(url.clone())),
        Value::Array(parts) if parts.len() == 3 => {
            let text = |v: &Value| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or("a fuzzy key part is not text")
            };
            Ok(FuzzyKey::Full(
                text(&parts[0])?,
                text(&parts[1])?,
                relation(&parts[2])?,
            ))
        }
        _ => Err(String::from(
            "a fuzzy key is not null, a url or [test, ref, relation]",
        )),
    }
}

fn ranges(v: &Value) -> Result<Fuzzy, String> {
    let pair = v
        .as_array()
        .filter(|p| p.len() == 2)
        .ok_or("fuzzy ranges are not [[min, max], [min, max]]")?;
    let range = |v: &Value| -> Result<(u64, u64), String> {
        let r = v
            .as_array()
            .filter(|r| r.len() == 2)
            .ok_or("a fuzzy range is not [min, max]")?;
        let n = |v: &Value| v.as_u64().ok_or("a fuzzy bound is not a whole number");
        let (lo, hi) = (n(&r[0])?, n(&r[1])?);
        if lo > hi {
            return Err(format!("a fuzzy range {lo}-{hi} runs backwards"));
        }
        Ok((lo, hi))
    };
    let (c0, c1) = range(&pair[0])?;
    Ok(Fuzzy {
        per_channel: (
            u32::try_from(c0).unwrap_or(u32::MAX),
            u32::try_from(c1).unwrap_or(u32::MAX),
        ),
        pixels: range(&pair[1])?,
    })
}

impl Reftest {
    /// The fuzzy allowance for comparing `from` (this test, or a reference
    /// on its chain) with `to` under `relation`: wptrunner's order, the full
    /// key, then the ref url, then the key for any reference. None means
    /// exact.
    pub fn fuzzy_for(&self, from: &str, to: &str, relation: Relation) -> Option<Fuzzy> {
        let full = self.fuzzy.iter().find(
            |(k, _)| matches!(k, FuzzyKey::Full(a, b, r) if a == from && b == to && *r == relation),
        );
        let by_ref = || {
            self.fuzzy
                .iter()
                .find(|(k, _)| matches!(k, FuzzyKey::Ref(b) if b == to))
        };
        let any = || self.fuzzy.iter().find(|(k, _)| *k == FuzzyKey::Any);
        full.or_else(by_ref).or_else(any).map(|(_, f)| *f)
    }
}
