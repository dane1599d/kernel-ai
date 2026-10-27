//! `cargo xtask licences check` (ADR 0009 D5, as far as it is decided): every
//! SPDX id in the tree and in the dependency graph is in the free class or an
//! approved exception, and has its text in `LICENSES/<id>.txt`; `LICENSES/`
//! holds no text that nothing uses.
//!
//! The rules come from `deny.toml`, so cargo-deny and this check agree: its
//! `allow` list is the free class, its `exceptions` name the approved crates,
//! and its `clarify` entries fix crates whose metadata is incomplete.
//!
//! The tree's ids are those in `SPDX-License-Identifier:` header lines of
//! tracked files, in `REUSE.toml` once it exists, and in the hand list of
//! `xtask/notices.json`. The dependency graph is
//! the whole of `cargo metadata`, host tools included. Where an expression
//! offers a choice, the alternative earliest in `allow` is taken.
//!
//! TODO(ADR 0009 D5): the rule that every tracked file carries our own header
//! (or matches `REUSE.toml`) is off until the headers and `REUSE.toml` land. The
//! licence is chosen (MPL-2.0, decided 2026-10-27); files with no header are
//! the workspace's MPL-2.0, `kabi` and `compsdk` say theirs in Cargo.toml.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Lines at the top of a file searched for its header.
const HEADER_LINES: usize = 5;
const HEADER: &str = "SPDX-License-Identifier:";

pub fn main(args: &[String]) -> Result<(), String> {
    match args {
        [task] if task == "check" => check(&crate::workspace_root()),
        _ => Err("usage: cargo xtask licences check".into()),
    }
}

/// What `deny.toml` allows.
#[derive(Debug, Default, PartialEq)]
pub struct Rules {
    /// The free class, in order of preference.
    allow: Vec<String>,
    /// Crate name to the extra ids it alone may use.
    exceptions: BTreeMap<String, Vec<String>>,
    /// Crate name to the expression that replaces its metadata.
    clarify: BTreeMap<String, String>,
}

impl Rules {
    fn allows(&self, krate: Option<&str>, id: &str) -> bool {
        self.allow.iter().any(|a| a == id)
            || krate
                .and_then(|k| self.exceptions.get(k))
                .is_some_and(|ids| ids.iter().any(|a| a == id))
    }

    /// Lower is preferred; exceptions come after the whole free class.
    fn rank(&self, id: &str) -> usize {
        self.allow
            .iter()
            .position(|a| a == id)
            .unwrap_or(self.allow.len())
    }
}

/// The rules in `deny.toml`.
pub fn rules(root: &Path) -> Result<Rules, String> {
    let deny = fs::read_to_string(root.join("deny.toml")).map_err(|e| format!("deny.toml: {e}"))?;
    parse_rules(&deny)
}

fn check(root: &Path) -> Result<(), String> {
    let rules = rules(root)?;
    let mut problems = Vec::new();
    // Who uses an expression, the crate an exception may name, the expression.
    let mut uses: Vec<(String, Option<String>, String)> = tree_expressions(root)?
        .into_iter()
        .map(|(path, expression)| (path, None, expression))
        .collect();
    for (name, krate, expression) in crate::notices::hand_expressions(root)? {
        uses.push((format!("xtask/notices.json: {name}"), krate, expression));
    }
    for (name, version, expression) in graph(root)? {
        match rules.clarify.get(&name).cloned().or(expression) {
            Some(e) => uses.push((format!("{name} {version}"), Some(name), e)),
            None => problems.push(format!("{name} {version}: no licence in its metadata")),
        }
    }
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (who, krate, expression) in uses {
        match choose(&expression, &rules, krate.as_deref()) {
            Ok(ids) => ids.into_iter().for_each(|id| {
                used.entry(id).or_default().insert(who.clone());
            }),
            Err(why) => problems.push(format!("{who}: `{expression}`: {why}")),
        }
    }
    let texts = license_texts(root)?;
    for (id, users) in &used {
        if !texts.contains(id) {
            let first = users.iter().next().map_or("", String::as_str);
            problems.push(format!("LICENSES/{id}.txt is missing (used by {first})"));
        }
    }
    for id in texts.difference(&used.keys().cloned().collect()) {
        problems.push(format!("LICENSES/{id}.txt is used by nothing"));
    }
    if !problems.is_empty() {
        return Err(format!(
            "licences check failed:\n  {}",
            problems.join("\n  ")
        ));
    }
    let ids: Vec<&str> = used.keys().map(String::as_str).collect();
    println!("xtask: licences ok: {}", ids.join(", "));
    Ok(())
}

/// The quoted strings in `allow`, `exceptions` and `clarify` of `deny.toml`:
/// only the shapes that file uses, one exception per line.
fn parse_rules(text: &str) -> Result<Rules, String> {
    let mut rules = Rules::default();
    let mut section = "";
    let mut list: Option<&str> = None;
    let mut clarified: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let strings = quoted(line);
        match (section, list) {
            ("[licenses]", None) if line.starts_with("allow") => list = Some("allow"),
            ("[licenses]", None) if line.starts_with("exceptions") => list = Some("exceptions"),
            ("[[licenses.clarify]]", _) if line.starts_with("crate") => {
                clarified = strings.first().cloned();
            }
            ("[[licenses.clarify]]", _) if line.starts_with("expression") => {
                let name = clarified
                    .take()
                    .ok_or("deny.toml: clarify without a crate")?;
                let expression = strings.first().ok_or("deny.toml: empty clarify")?;
                rules.clarify.insert(name, expression.clone());
            }
            _ => {}
        }
        match list {
            Some("allow") => rules.allow.extend(strings),
            Some(_) if line.starts_with('{') => match strings.split_first() {
                Some((name, ids)) => {
                    rules.exceptions.insert(name.clone(), ids.to_vec());
                }
                None => return Err(format!("deny.toml: bad exception `{line}`")),
            },
            _ => {}
        }
        if line.ends_with(']') && !line.starts_with('{') {
            list = None;
        }
    }
    if rules.allow.is_empty() {
        return Err("deny.toml: no [licenses] allow list".into());
    }
    Ok(rules)
}

/// The double-quoted strings on a line, outside a `#` comment.
fn quoted(line: &str) -> Vec<String> {
    let line = line.split(" #").next().unwrap_or(line);
    if line.starts_with('#') {
        return Vec::new();
    }
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The ids `expression` needs under `rules`: the preferred alternative of each
/// `OR`, every operand of each `AND`, both halves of a `WITH`.
pub fn choose(expression: &str, rules: &Rules, krate: Option<&str>) -> Result<Vec<String>, String> {
    let spaced = expression
        .replace('(', " ( ")
        .replace(')', " ) ")
        .replace('/', " OR ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut at = 0;
    let chosen = any_of(&tokens, &mut at, rules, krate)?;
    if at != tokens.len() {
        return Err(format!("unexpected `{}`", tokens[at]));
    }
    chosen.ok_or_else(|| "no alternative is in the free class or approved for it".into())
}

type Choice = Result<Option<Vec<String>>, String>;

fn any_of(tokens: &[&str], at: &mut usize, rules: &Rules, krate: Option<&str>) -> Choice {
    let mut best: Option<Vec<String>> = None;
    loop {
        if let Some(ids) = all_of(tokens, at, rules, krate)? {
            let worst = |ids: &[String]| ids.iter().map(|id| rules.rank(id)).max();
            if best.as_ref().is_none_or(|b| worst(&ids) < worst(b)) {
                best = Some(ids);
            }
        }
        if tokens.get(*at) != Some(&"OR") {
            return Ok(best);
        }
        *at += 1;
    }
}

fn all_of(tokens: &[&str], at: &mut usize, rules: &Rules, krate: Option<&str>) -> Choice {
    let mut all = Some(Vec::new());
    loop {
        let one = term(tokens, at, rules, krate)?;
        all = all.zip(one).map(|(mut a, b)| {
            a.extend(b);
            a
        });
        if tokens.get(*at) != Some(&"AND") {
            return Ok(all);
        }
        *at += 1;
    }
}

fn term(tokens: &[&str], at: &mut usize, rules: &Rules, krate: Option<&str>) -> Choice {
    let token = *tokens.get(*at).ok_or("ends early")?;
    *at += 1;
    if token == "(" {
        let inner = any_of(tokens, at, rules, krate)?;
        if tokens.get(*at) != Some(&")") {
            return Err("unclosed `(`".into());
        }
        *at += 1;
        return Ok(inner);
    }
    if matches!(token, ")" | "AND" | "OR" | "WITH") {
        return Err(format!("unexpected `{token}`"));
    }
    // `MPL-2.0+` is "or later": the named version satisfies it.
    let id = token.trim_end_matches('+');
    if tokens.get(*at) == Some(&"WITH") {
        let exception = *tokens.get(*at + 1).ok_or("ends after WITH")?;
        *at += 2;
        let whole = format!("{id} WITH {exception}");
        return Ok(rules
            .allows(krate, &whole)
            .then(|| vec![id.to_owned(), exception.to_owned()]));
    }
    Ok(rules.allows(krate, id).then(|| vec![id.to_owned()]))
}

/// Header expressions of tracked files, and `REUSE.toml`'s, by path.
fn tree_expressions(root: &Path) -> Result<Vec<(String, String)>, String> {
    let out = run(root, "git", &["ls-files", "-z"])?;
    let mut found = Vec::new();
    for path in out.split('\0').filter(|p| !p.is_empty()) {
        let Ok(text) = fs::read_to_string(root.join(path)) else {
            continue; // not UTF-8 text: covered by REUSE.toml once it exists
        };
        let lines = text.lines().take(HEADER_LINES);
        found.extend(lines.filter_map(header).map(|e| (path.to_owned(), e)));
    }
    if let Ok(reuse) = fs::read_to_string(root.join("REUSE.toml")) {
        for line in reuse
            .lines()
            .filter(|l| l.trim_start().starts_with(HEADER.trim_end_matches(':')))
        {
            found.extend(
                quoted(line)
                    .into_iter()
                    .map(|e| ("REUSE.toml".to_owned(), e)),
            );
        }
    }
    Ok(found)
}

/// The expression of a header line such as `// SPDX-License-Identifier: MIT`.
fn header(line: &str) -> Option<String> {
    let rest = line.trim_start_matches(|c: char| c.is_whitespace() || "/#*;-<!".contains(c));
    let expression = rest.strip_prefix(HEADER)?;
    let expression = expression
        .trim()
        .trim_end_matches("-->")
        .trim_end_matches("*/")
        .trim();
    Some(expression.to_owned())
}

/// Every package in the dependency graph but our own: name, version, licence.
fn graph(root: &Path) -> Result<Vec<(String, String, Option<String>)>, String> {
    let out = run(
        root,
        "cargo",
        &["metadata", "--format-version", "1", "--locked"],
    )?;
    let metadata: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("cargo metadata: {e}"))?;
    let ours: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata: no workspace members")?
        .iter()
        .filter_map(|m| m.as_str())
        .collect();
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata: no packages")?;
    Ok(packages
        .iter()
        .filter(|p| !p["id"].as_str().is_some_and(|id| ours.contains(id)))
        .map(|p| {
            let field = |k: &str| p[k].as_str().map(str::to_owned);
            (
                field("name").unwrap_or_default(),
                field("version").unwrap_or_default(),
                field("license"),
            )
        })
        .collect())
}

/// The ids with a text in `LICENSES/`.
fn license_texts(root: &Path) -> Result<BTreeSet<String>, String> {
    let Ok(entries) = fs::read_dir(root.join("LICENSES")) else {
        return Ok(BTreeSet::new());
    };
    let mut ids = BTreeSet::new();
    for entry in entries {
        let name = entry.map_err(|e| format!("LICENSES: {e}"))?.file_name();
        let name = name.to_string_lossy();
        match name.strip_suffix(".txt") {
            Some(id) => ids.insert(id.to_owned()),
            None => return Err(format!("LICENSES/{name}: not an <id>.txt licence text")),
        };
    }
    Ok(ids)
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{program} {}: {err}", args.join(" ")));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        parse_rules(
            r#"
[licenses]
allow = [
    "MIT",
    "Apache-2.0", # a comment
    "Apache-2.0 WITH LLVM-exception",
    "0BSD",
]
exceptions = [
    # approved
    { crate = "smartstring", allow = ["MPL-2.0"] },
]

[[licenses.clarify]]
crate = "noto-sans-mono-bitmap"
expression = "MIT AND OFL-1.1"
license-files = [{ path = "LICENSE", hash = 0x1 }]

[advisories]
ignore = [{ id = "X", reason = "y" }]
"#,
        )
        .unwrap()
    }

    #[test]
    fn deny_toml_rules_are_read() {
        let r = rules();
        assert_eq!(
            r.allow,
            [
                "MIT",
                "Apache-2.0",
                "Apache-2.0 WITH LLVM-exception",
                "0BSD"
            ]
        );
        assert_eq!(r.exceptions["smartstring"], ["MPL-2.0"]);
        assert_eq!(r.clarify["noto-sans-mono-bitmap"], "MIT AND OFL-1.1");
        assert_eq!(r.exceptions.len(), 1);
    }

    #[test]
    fn the_preferred_alternative_is_chosen() {
        let r = rules();
        let pick = |e: &str| choose(e, &r, None);
        assert_eq!(pick("MIT OR Apache-2.0").unwrap(), ["MIT"]);
        assert_eq!(pick("Apache-2.0/MIT").unwrap(), ["MIT"]);
        assert_eq!(pick("0BSD OR MIT OR Apache-2.0").unwrap(), ["MIT"]);
        assert_eq!(
            pick("Apache-2.0 WITH LLVM-exception OR GPL-3.0").unwrap(),
            ["Apache-2.0", "LLVM-exception"]
        );
        assert_eq!(pick("(MIT OR GPL-2.0) AND 0BSD").unwrap(), ["MIT", "0BSD"]);
    }

    #[test]
    fn licences_outside_the_rules_are_refused() {
        let r = rules();
        assert!(choose("GPL-3.0", &r, None).is_err());
        assert!(choose("MIT AND LGPL-2.1", &r, None).is_err());
        assert!(choose("Apache-2.0 WITH Classpath-exception-2.0", &r, None).is_err());
        // An exception covers its own crate only.
        assert!(choose("MPL-2.0+", &r, None).is_err());
        assert!(choose("MPL-2.0", &r, Some("rhai")).is_err());
        assert_eq!(
            choose("MPL-2.0+", &r, Some("smartstring")).unwrap(),
            ["MPL-2.0"]
        );
        for broken in ["MIT OR", "(MIT", "MIT)", "AND MIT", ""] {
            assert!(choose(broken, &r, None).is_err(), "{broken}");
        }
    }

    #[test]
    fn header_lines_are_read_in_any_comment_syntax() {
        assert_eq!(
            header("// SPDX-License-Identifier: MIT").as_deref(),
            Some("MIT")
        );
        assert_eq!(
            header("# SPDX-License-Identifier: 0BSD").as_deref(),
            Some("0BSD")
        );
        assert_eq!(
            header("<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->").as_deref(),
            Some("MIT OR Apache-2.0")
        );
        assert_eq!(
            header("/* SPDX-License-Identifier: MIT */").as_deref(),
            Some("MIT")
        );
        assert_eq!(header("the `SPDX-License-Identifier:` line"), None);
    }
}
