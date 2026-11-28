//! A lint over `.github/workflows/*.yml` and `.github/actions/*/action.yml`: what
//! the Actions cache may save, and what a release tag may restore (bug hunt 5,
//! H5-1 and H5-3).
//!
//! The store has a 10 GB cap, and an entry saved on a pull request or a tag
//! serves only that ref, so caches are saved from main only (61f21c9d): an
//! `actions/cache/save` step's `if:` names `refs/heads/main`, a full
//! `actions/cache` step (which saves in its post step wherever it runs) is
//! `lookup-only`, and `Swatinem/rust-cache` sets `save-if` to main or `false`.
//! Small entries a run's own later jobs restore (QEMU, tuftool) may be saved
//! anywhere. On a tag, release.yml restores nothing, nor does any workflow or
//! local action it uses (release-images.yml, [`TAG_RUN`]): no cache restore
//! without an `if:` that is false on a tag (an `&&` chain with a not-a-tag
//! term, [`not_on_a_tag`]), rust-cache included, which restores whatever its
//! save-if.
//! The jobs `publish` needs would otherwise run what any job on main could
//! have written there.
//!
//! Two more rules guard what a workflow may do. Every workflow has a top-level
//! `permissions:` block (without one it gets the repository's default token,
//! which may write), and only `release.yml` asks for a `write` scope. Every
//! `uses:` of a remote action names a full commit sha, never a tag a maintainer
//! can move (the tag goes in a trailing comment).
//!
//! The workflows are read line by line, not as YAML: a step is a `- ` list item
//! and its lines, and its fields are the lines one indent below the dash.

use std::fs;
use std::path::{Path, PathBuf};

/// Keys of entries small enough to save from any ref.
const SMALL: &[&str] = &["qemu-", "tuftool-"];
const MAIN: &str = "github.ref == 'refs/heads/main'";
const NOT_A_TAG: &str = "!startsWith(github.ref, 'refs/tags/";
/// The files a release tag runs, from the repository root: release.yml, each
/// workflow or local action it uses (`uses: ./...`), and each file those use in
/// turn (a test walks the uses from release.yml and holds the list to them).
const TAG_RUN: &[&str] = &[
    ".github/workflows/release.yml",
    ".github/workflows/release-images.yml",
];

/// One `- ` list item: its first line's number and its lines, the dash
/// replaced by spaces.
struct Step {
    line: usize,
    indent: usize,
    lines: Vec<String>,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn steps(text: &str) -> Vec<Step> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let dash = indent(line);
        let mut block = vec![format!("{}{rest}", " ".repeat(dash + 2))];
        for next in &lines[i + 1..] {
            if !next.trim().is_empty() && indent(next) <= dash {
                break;
            }
            block.push(next.to_string());
        }
        out.push(Step {
            line: i + 1,
            indent: dash + 2,
            lines: block,
        });
    }
    out
}

impl Step {
    /// The value of the first `name:` whose indent `at` accepts, a folded or
    /// literal value's lines joined, any `# comment` after a plain one dropped.
    fn value(&self, name: &str, at: impl Fn(usize) -> bool) -> Option<String> {
        let key = format!("{name}:");
        let i = self
            .lines
            .iter()
            .position(|l| at(indent(l)) && l.trim_start().starts_with(&key))?;
        let own = indent(&self.lines[i]);
        let value = self.lines[i].trim_start()[key.len()..].trim();
        if matches!(value, ">" | ">-" | "|" | "|-") {
            let more = self.lines[i + 1..]
                .iter()
                .take_while(|l| l.trim().is_empty() || indent(l) > own)
                .map(|l| l.trim())
                .collect::<Vec<_>>();
            return Some(more.join(" ").trim().to_string());
        }
        let value = value.split(" #").next().unwrap_or(value).trim();
        Some(value.to_string())
    }

    /// A field of the step itself (`uses`, `if`).
    fn field(&self, name: &str) -> Option<String> {
        self.value(name, |i| i == self.indent)
    }

    /// An input under the step's `with:`.
    fn input(&self, name: &str) -> Option<String> {
        self.value(name, |i| i > self.indent)
    }
}

/// What the rules find wrong in one file, as `file:line: why`, and how many
/// cache steps it read.
fn lint(file: &str, text: &str) -> (Vec<String>, usize) {
    let release = TAG_RUN.contains(&file);
    let (mut errors, mut seen) = (Vec::new(), 0);
    for step in steps(text) {
        let Some(uses) = step.field("uses") else {
            continue;
        };
        let at = format!("{file}:{}", step.line);
        let cond = step.field("if").unwrap_or_default();
        let key = step.input("key").unwrap_or_default();
        let small = SMALL.iter().any(|p| key.starts_with(p));
        let full = uses.starts_with("actions/cache@");
        let restore = full || uses.starts_with("actions/cache/restore@");
        let save = uses.starts_with("actions/cache/save@");
        if uses.starts_with("Swatinem/rust-cache@") {
            seen += 1;
            let save_if = step.input("save-if").unwrap_or_default();
            if save_if != "false" && !save_if.contains(MAIN) {
                errors.push(format!(
                    "{at}: rust-cache saves from any ref: set save-if to `{MAIN}` or false"
                ));
            }
            // rust-cache restores on every ref, whatever save-if says.
            if release && !not_on_a_tag(&cond) {
                errors.push(format!(
                    "{at}: a release tag restores rust-cache: add `if: ${{{{ {NOT_A_TAG}') }}}}`"
                ));
            }
            continue;
        }
        if !(restore || save) {
            continue;
        }
        seen += 1;
        let lookup = step.input("lookup-only").is_some_and(|v| v == "true");
        if full && !lookup && !small {
            errors.push(format!(
                "{at}: actions/cache saves `{key}` from any ref: use actions/cache/restore, \
                 and actions/cache/save under `if: {MAIN}`"
            ));
        }
        if save && !small && !cond.contains(MAIN) {
            errors.push(format!(
                "{at}: actions/cache/save saves `{key}` from any ref: its if must name `{MAIN}`"
            ));
        }
        if release && restore && !lookup && !not_on_a_tag(&cond) {
            errors.push(format!(
                "{at}: a release tag restores `{key}`: add `if: ${{{{ {NOT_A_TAG}') }}}}`"
            ));
        }
    }
    (errors, seen)
}

/// Whether `cond` is false on a tag whatever else holds. With `${{ }}` and
/// outer parentheses taken off it must be one term or a chain of terms joined
/// by `&&`, with no `||` at the top level, and one term must be
/// `!startsWith(github.ref, 'refs/tags/...')` or `github.ref_type != 'tag'`.
/// A substring match is not enough: `always() || !startsWith(...)` and
/// `!startsWith(...) || x` both run on a tag. Anything else (a negated group,
/// say) counts as running on a tag.
fn not_on_a_tag(cond: &str) -> bool {
    let cond = cond.trim();
    let cond = cond
        .strip_prefix("${{")
        .and_then(|c| c.strip_suffix("}}"))
        .unwrap_or(cond);
    // Spaces outside quotes carry no meaning in an expression.
    let mut quoted = false;
    let cond: String = cond
        .chars()
        .filter(|&c| {
            quoted ^= c == '\'';
            quoted || !c.is_whitespace()
        })
        .collect();
    let Some(terms) = and_terms(unwrap_parens(&cond)) else {
        return false;
    };
    terms.into_iter().map(unwrap_parens).any(|t| {
        (t.starts_with(&NOT_A_TAG.replace(' ', "")) && t.ends_with("')"))
            || t == "github.ref_type!='tag'"
            || t == "'tag'!=github.ref_type"
    })
}

/// `expr` without parentheses that wrap all of it.
fn unwrap_parens(mut expr: &str) -> &str {
    while let Some(inner) = expr.strip_prefix('(').and_then(|e| e.strip_suffix(')')) {
        // `(a) && (b)` starts and ends with one, but they are not a pair.
        let mut depth = 0i32;
        let pair = inner.chars().all(|c| {
            depth += i32::from(c == '(') - i32::from(c == ')');
            depth >= 0
        });
        if !pair {
            break;
        }
        expr = inner;
    }
    expr
}

/// The terms of `expr` split on `&&` outside parentheses and quotes, or None
/// when an `||` is there too.
fn and_terms(expr: &str) -> Option<Vec<&str>> {
    let (bytes, mut terms, mut start) = (expr.as_bytes(), Vec::new(), 0);
    let (mut depth, mut quoted, mut i) = (0i32, false, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => quoted = !quoted,
            b'(' if !quoted => depth += 1,
            b')' if !quoted => depth -= 1,
            b'|' if !quoted && depth == 0 && bytes.get(i + 1) == Some(&b'|') => return None,
            b'&' if !quoted && depth == 0 && bytes.get(i + 1) == Some(&b'&') => {
                terms.push(&expr[start..i]);
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    terms.push(&expr[start..]);
    Some(terms)
}

/// The local files `text` uses, from the repository root: `uses:
/// ./.github/workflows/x.yml` is that file and `uses: ./.github/actions/x` that
/// action's `action.yml`, a job's or a step's, quoted or not.
fn local_uses(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let body = line.trim_start();
            let body = body.strip_prefix("- ").unwrap_or(body).trim_start();
            let uses = body.strip_prefix("uses:")?;
            let uses = uses.split(" #").next().unwrap_or(uses).trim();
            let uses = uses.trim_matches(|c| c == '"' || c == '\'');
            let path = uses.strip_prefix("./")?.trim_end_matches('/');
            Some(match path.ends_with(".yml") || path.ends_with(".yaml") {
                true => path.to_string(),
                false => format!("{path}/action.yml"),
            })
        })
        .collect()
}

/// Every file a run of `start` reaches through local uses, `start` first, each
/// once, so a cycle ends; `read` gives a file's text by its path.
fn reached(start: &str, read: impl Fn(&str) -> String) -> Vec<String> {
    let (mut todo, mut seen) = (vec![start.to_string()], Vec::<String>::new());
    while let Some(file) = todo.pop() {
        if seen.contains(&file) {
            continue;
        }
        todo.extend(local_uses(&read(&file)).into_iter().rev());
        seen.push(file);
    }
    seen
}

/// The workflows that may ask for a `write` permission.
const MAY_WRITE: &[&str] = &["release.yml"];

fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A value after `scope:` that grants write: `write`, `write-all`, either
/// quoted, a `# comment` after it, or a flow mapping naming a `write` scope.
fn grants_write(value: &str) -> bool {
    let value = value.split(" #").next().unwrap_or(value).trim();
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    if let Some(flow) = value.strip_prefix('{') {
        return flow
            .trim_end_matches('}')
            .split(',')
            .any(|pair| pair.split_once(':').is_some_and(|(_, v)| grants_write(v)));
    }
    matches!(value, "write" | "write-all")
}

/// A `permissions:` key at column 0 whose value is empty, a flow mapping, or
/// `read-all` / `write-all` (what a top-level block can be), not another key
/// that starts with the word or a line of a scalar. A block written as a
/// multi-line flow mapping, or with an anchor (`permissions: &p`), is not
/// recognised and fails the lint: it fails safe, and the fix is the plain block.
fn top_permissions(line: &str) -> bool {
    let Some(value) = line.strip_prefix("permissions:") else {
        return false;
    };
    let value = value.split(" #").next().unwrap_or(value).trim();
    matches!(value, "" | "read-all" | "write-all")
        || (value.starts_with('{') && value.ends_with('}'))
}

/// What the policy rules find wrong in one file: `workflow` is true for a file
/// under `.github/workflows` (a local action has no `permissions:`).
fn policy(file: &str, text: &str, workflow: bool) -> Vec<String> {
    let name = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file);
    let mut errors = Vec::new();
    if workflow && !text.lines().any(top_permissions) {
        errors.push(format!(
            "{file}:1: no top-level `permissions:` block: add `permissions: {{}}` or the scopes it needs"
        ));
    }
    for (i, line) in text.lines().enumerate() {
        let at = format!("{file}:{}", i + 1);
        let body = line.trim_start();
        let body = body.strip_prefix("- ").unwrap_or(body).trim_start();
        if body.starts_with('#') {
            continue;
        }
        if let Some((scope, value)) = body.split_once(':')
            && !scope.contains(' ')
            && grants_write(value)
            && !MAY_WRITE.contains(&name)
        {
            errors.push(format!(
                "{at}: `{scope}: {}` outside {MAY_WRITE:?}",
                value.trim()
            ));
        }
        let Some(uses) = body.strip_prefix("uses:") else {
            continue;
        };
        let uses = uses.split(" #").next().unwrap_or(uses).trim();
        if uses.starts_with("./") {
            continue;
        }
        // A container is pinned by its image digest, an action by a commit sha.
        let pinned = match uses.strip_prefix("docker://") {
            Some(image) => image
                .rsplit_once("@sha256:")
                .is_some_and(|(_, d)| hex(d, 64)),
            None => uses.rsplit_once('@').is_some_and(|(_, sha)| hex(sha, 40)),
        };
        if !pinned {
            errors.push(format!(
                "{at}: `{uses}` is not pinned to a full commit sha: use `@<sha> # <tag>`"
            ));
        }
    }
    errors
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every workflow, and every local action's action.yml.
fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let yaml = |p: &Path| p.extension().is_some_and(|e| e == "yml" || e == "yaml");
    let workflows = root().join(".github/workflows");
    for entry in fs::read_dir(&workflows).expect(".github/workflows") {
        let path = entry.expect("a workflow").path();
        if yaml(&path) {
            out.push(path);
        }
    }
    let actions = root().join(".github/actions");
    for entry in fs::read_dir(&actions).expect(".github/actions") {
        let dir = entry.expect("an action").path();
        for name in ["action.yml", "action.yaml"] {
            if dir.join(name).is_file() {
                out.push(dir.join(name));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn caches_save_from_main_only_and_a_release_tag_restores_none() {
    let (mut errors, mut seen) = (Vec::new(), 0);
    for path in files() {
        let text = fs::read_to_string(&path).expect("a workflow file");
        let name = path
            .strip_prefix(root())
            .unwrap_or(&path)
            .display()
            .to_string();
        let (e, n) = lint(&name, &text);
        errors.extend(e);
        seen += n;
        errors.extend(policy(
            &name,
            &text,
            path.starts_with(root().join(".github/workflows")),
        ));
    }
    // The walk reads the cache steps: ci.yml alone has more than ten.
    assert!(seen > 20, "read only {seen} cache steps");
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn the_lint_refuses_each_case_it_names() {
    let bad = "\
jobs:
  a:
    steps:
      - name: A full cache of a model, saved from any ref
        uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4
        with:
          path: ~/m
          key: model-chunks-1
      - uses: actions/cache/save@v4
        if: steps.c.outputs.cache-hit != 'true'
        with:
          key: model-2
      - uses: Swatinem/rust-cache@v2
      - uses: Swatinem/rust-cache@v2
        with:
          shared-key: x
          save-if: ${{ matrix.shard == 1 }}
      - id: qemu
        uses: actions/cache/restore@v4
        with:
          key: qemu-11.0.5-ubuntu-24.04-v1
";
    let (errors, seen) = lint(".github/workflows/release.yml", bad);
    let lines: Vec<&str> = errors
        .iter()
        .map(|e| e.split(": ").next().unwrap_or(""))
        .collect();
    let at = |n: usize| format!(".github/workflows/release.yml:{n}");
    // Line 4's full cache both saves anywhere and is restored on a tag, and
    // so do lines 13 and 14's rust-caches.
    assert_eq!(
        lines,
        [at(4), at(4), at(9), at(13), at(13), at(14), at(14), at(18)],
        "{errors:#?}"
    );
    assert_eq!(seen, 5);

    let good = "\
runs:
  using: composite
  steps:
    - id: c
      if: ${{ !startsWith(github.ref, 'refs/tags/') }}
      uses: actions/cache/restore@v4
      with:
        key: model-1
    - name: Saved from main only
      if: >-
        github.ref == 'refs/heads/main' &&
        steps.c.outputs.cache-hit != 'true'
      uses: actions/cache/save@v4
      with:
        key: model-1
    - uses: actions/cache@v4
      with:
        key: model-1
        lookup-only: true
    - uses: actions/cache@v4 # v4
      if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
      with:
        key: tuftool-0.17.0-Linux
    - uses: actions/cache/save@v4
      with:
        key: qemu-11.0.5-ubuntu-24.04-v1
    - uses: Swatinem/rust-cache@v2
      if: ${{ !startsWith(github.ref, 'refs/tags/') }}
      with:
        save-if: ${{ github.ref == 'refs/heads/main' && matrix.shard == 1 }}
    - uses: Swatinem/rust-cache@v2
      if: ${{ !startsWith(github.ref, 'refs/tags/') }}
      with:
        save-if: false
";
    let (errors, seen) = lint(".github/workflows/release.yml", good);
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(seen, 7);
}

#[test]
fn the_policy_refuses_a_missing_block_a_write_and_a_tag() {
    let bad = "\
name: x
on: push
jobs:
  a:
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: some/action@main # v1
      - uses: ./.github/actions/local
      - uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4
";
    let errors = policy(".github/workflows/ci.yml", bad, true);
    let lines: Vec<&str> = errors
        .iter()
        .map(|e| e.split(':').nth(1).unwrap_or(""))
        .collect();
    assert_eq!(lines, ["1", "6", "8", "9"], "{errors:#?}");
    // The same file is a release's: its write is allowed, the rest is not.
    let release = policy(".github/workflows/release.yml", bad, true);
    assert_eq!(release.len(), 3, "{release:#?}");
    // A local action has no permissions block to ask for.
    let local = "runs:\n  using: composite\n  steps:\n    - uses: actions/checkout@v4\n";
    assert_eq!(
        policy(".github/actions/x/action.yml", local, false).len(),
        1
    );

    let good = "\
permissions: {}
jobs:
  a:
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
";
    assert!(policy(".github/workflows/ci.yml", good, true).is_empty());

    // Every spelling of a write is caught: write-all, quotes, a trailing
    // comment, a flow mapping.
    for line in [
        "permissions: write-all",
        "  contents: \"write\"",
        "  contents: 'write'",
        "  contents: write # needed to tag",
        "  permissions: { contents: write }",
        "  permissions: { contents: read, issues: write }",
    ] {
        let text = format!("permissions: {{}}\njobs:\n{line}\n");
        let found = policy(".github/workflows/ci.yml", &text, true);
        assert_eq!(found.len(), 1, "{line}: {found:#?}");
    }
    for line in [
        "  contents: read",
        "  contents: write-ish-name",
        "permissions: read-all",
    ] {
        let text = format!("permissions: {{}}\njobs:\n{line}\n");
        assert!(
            policy(".github/workflows/ci.yml", &text, true).is_empty(),
            "{line}"
        );
    }

    // Only a real top-level block counts: not a longer key or a stray value.
    for head in [
        "permissionsx: {}",
        "permissions:foo",
        "permissions: contents",
    ] {
        let text = format!("{head}\njobs: {{}}\n");
        assert_eq!(
            policy(".github/workflows/ci.yml", &text, true).len(),
            1,
            "{head}"
        );
    }
    let block = "permissions:\n  contents: read\njobs: {}\n";
    assert!(policy(".github/workflows/ci.yml", block, true).is_empty());

    // A container image is pinned by its digest.
    let digest = "a".repeat(64);
    let image = format!(
        "permissions: {{}}\njobs:\n  a:\n    steps:\n      - uses: docker://alpine@sha256:{digest}\n"
    );
    assert!(policy(".github/workflows/ci.yml", &image, true).is_empty());
    let tag = "permissions: {}\njobs:\n  a:\n    steps:\n      - uses: docker://alpine:3\n";
    assert_eq!(policy(".github/workflows/ci.yml", tag, true).len(), 1);
}

/// A release tag runs every file release.yml uses, and every file those use,
/// so each is in [`TAG_RUN`], and the no-restore rule holds in a
/// called one as in release.yml itself.
#[test]
fn a_workflow_release_yml_calls_restores_nothing_on_a_tag() {
    let read = |file: &str| {
        let path = root().join(file);
        let yaml = path.with_file_name("action.yaml");
        fs::read_to_string(&path)
            .or_else(|_| fs::read_to_string(yaml))
            .unwrap_or_else(|e| panic!("{file}: {e}"))
    };
    let seen = reached(".github/workflows/release.yml", read);
    for file in &seen {
        assert!(
            TAG_RUN.contains(&file.as_str()),
            "a tag runs {file}, not in TAG_RUN"
        );
    }
    assert!(
        seen.iter()
            .any(|f| f == ".github/workflows/release-images.yml"),
        "{seen:?}"
    );
    let restore = "\
jobs:
  a:
    steps:
      - uses: actions/cache/restore@v4
        with:
          key: model-1
";
    let (errors, _) = lint(".github/workflows/release-images.yml", restore);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].starts_with(".github/workflows/release-images.yml:4: a release tag restores")
    );
    let (errors, _) = lint(".github/workflows/repro.yml", restore);
    assert!(errors.is_empty(), "{errors:#?}");

    // rust-cache restores on any ref, so on a tag it needs the not-a-tag if too.
    let rust_cache = |cond: &str| {
        format!(
            "jobs:\n  a:\n    steps:\n      - uses: Swatinem/rust-cache@v2\n{cond}        with:\n          save-if: ${{{{ {MAIN} }}}}\n"
        )
    };
    let (errors, _) = lint(".github/workflows/release-images.yml", &rust_cache(""));
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].contains("a release tag restores rust-cache"),
        "{errors:#?}"
    );
    let guarded = rust_cache(&format!("        if: ${{{{ {NOT_A_TAG}') }}}}\n"));
    let (errors, _) = lint(".github/workflows/release-images.yml", &guarded);
    assert!(errors.is_empty(), "{errors:#?}");
    let (errors, _) = lint(".github/workflows/repro.yml", &rust_cache(""));
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The walk goes through nested uses, into local actions, past quotes, and
/// ends on a cycle, each file once.
#[test]
fn the_tag_walk_follows_nested_uses_and_local_actions_and_ends_on_a_cycle() {
    let files = [
        (
            ".github/workflows/r.yml",
            "jobs:\n  a:\n    uses: ./.github/workflows/b.yml\n",
        ),
        (
            ".github/workflows/b.yml",
            "jobs:\n  b:\n    steps:\n      - uses: \"./.github/actions/q\" # quoted\n      - uses: './.github/workflows/c.yml'\n",
        ),
        (
            ".github/actions/q/action.yml",
            "runs:\n  steps:\n    - uses: ./.github/actions/q/\n",
        ),
        (
            ".github/workflows/c.yml",
            "jobs:\n  c:\n    uses: ./.github/workflows/r.yml\n",
        ),
    ];
    let read = |f: &str| {
        files
            .iter()
            .find(|(p, _)| *p == f)
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| panic!("read {f}"))
    };
    assert_eq!(
        reached(".github/workflows/r.yml", read),
        [
            ".github/workflows/r.yml",
            ".github/workflows/b.yml",
            ".github/actions/q/action.yml",
            ".github/workflows/c.yml",
        ]
    );
}

/// The not-a-tag `if:` is parsed, not matched as a substring: only an `&&`
/// chain with a not-a-tag term is false on every tag.
#[test]
fn only_an_and_chain_with_a_not_a_tag_term_keeps_a_step_off_a_tag() {
    for cond in [
        "${{ !startsWith(github.ref, 'refs/tags/') }}",
        "!startsWith(github.ref,'refs/tags/v')",
        "steps.c.outputs.cache-hit != 'true' && !startsWith(github.ref, 'refs/tags/')",
        "${{ always() && github.ref_type != 'tag' }}",
        "('tag' != github.ref_type)",
        "(github.event_name == 'push') && (!startsWith(github.ref, 'refs/tags/'))",
    ] {
        assert!(not_on_a_tag(cond), "{cond}");
    }
    for cond in [
        "",
        "always()",
        "${{ always() || !startsWith(github.ref, 'refs/tags/') }}",
        "!startsWith(github.ref, 'refs/tags/') || github.event_name == 'push'",
        "github.ref_type == 'tag'",
        "github.ref_type != 'tag' || always()",
        "!(startsWith(github.ref, 'refs/tags/') || always())",
        "(!startsWith(github.ref, 'refs/tags/')) || true",
        "contains(github.ref, 'x || y') || github.ref_type != 'tag'",
    ] {
        assert!(!not_on_a_tag(cond), "{cond}");
    }
    // A `||` inside quotes or parentheses does not break the chain.
    assert!(not_on_a_tag(
        "contains(github.ref, 'x || y') && (a || b) && github.ref_type != 'tag'"
    ));

    // And the lint holds a rust-cache and a restore in a TAG_RUN file to it.
    let step = |uses: &str, cond: &str| {
        format!(
            "jobs:\n  a:\n    steps:\n      - uses: {uses}\n        if: {cond}\n        with:\n          save-if: false\n          key: model-1\n"
        )
    };
    let file = ".github/workflows/release-images.yml";
    for uses in ["Swatinem/rust-cache@v2", "actions/cache/restore@v4"] {
        for cond in [
            "${{ always() || !startsWith(github.ref, 'refs/tags/') }}",
            "${{ !startsWith(github.ref, 'refs/tags/') || always() }}",
        ] {
            let (errors, _) = lint(file, &step(uses, cond));
            assert_eq!(errors.len(), 1, "{uses} {cond}: {errors:#?}");
            assert!(errors[0].contains("a release tag restores"), "{errors:#?}");
        }
        let (errors, _) = lint(file, &step(uses, "${{ github.ref_type != 'tag' }}"));
        assert!(errors.is_empty(), "{uses}: {errors:#?}");
    }
}
