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
//! anywhere. On a tag, release.yml restores nothing, nor does any workflow it
//! calls (release-images.yml, [`TAG_RUN`]): no cache restore without the
//! not-a-tag `if:`, rust-cache included, which restores whatever its save-if.
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
/// The workflows a release tag runs: release.yml and each workflow it calls
/// with `uses: ./.github/workflows/...`, and each those call (a test follows
/// the calls and holds the list to them).
const TAG_RUN: &[&str] = &["release.yml", "release-images.yml"];

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
    let release = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| TAG_RUN.contains(&n));
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
            if release && !cond.contains(NOT_A_TAG) {
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
        if release && restore && !lookup && !cond.contains(NOT_A_TAG) {
            errors.push(format!(
                "{at}: a release tag restores `{key}`: add `if: ${{{{ {NOT_A_TAG}') }}}}`"
            ));
        }
    }
    (errors, seen)
}

/// The workflows that may ask for a `write` permission.
const MAY_WRITE: &[&str] = &["release.yml"];

/// What the policy rules find wrong in one file: `workflow` is true for a file
/// under `.github/workflows` (a local action has no `permissions:`).
fn policy(file: &str, text: &str, workflow: bool) -> Vec<String> {
    let name = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file);
    let mut errors = Vec::new();
    if workflow && !text.lines().any(|l| l.starts_with("permissions:")) {
        errors.push(format!(
            "{file}:1: no top-level `permissions:` block: add `permissions: {{}}` or the scopes it needs"
        ));
    }
    for (i, line) in text.lines().enumerate() {
        let at = format!("{file}:{}", i + 1);
        let body = line.trim_start();
        let body = body.strip_prefix("- ").unwrap_or(body).trim_start();
        if let Some((scope, "write")) = body.split_once(": ")
            && !scope.contains(' ')
            && !MAY_WRITE.contains(&name)
        {
            errors.push(format!("{at}: `{scope}: write` outside {MAY_WRITE:?}"));
        }
        let Some(uses) = body.strip_prefix("uses:") else {
            continue;
        };
        let uses = uses.split(" #").next().unwrap_or(uses).trim();
        if uses.starts_with("./") {
            continue;
        }
        let pinned = uses
            .rsplit_once('@')
            .is_some_and(|(_, sha)| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()));
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
}

/// A release tag runs every workflow release.yml calls, and every workflow
/// those call, so each is in [`TAG_RUN`], and the no-restore rule holds in a
/// called one as in release.yml itself.
#[test]
fn a_workflow_release_yml_calls_restores_nothing_on_a_tag() {
    let dir = root().join(".github/workflows");
    let (mut todo, mut seen) = (vec!["release.yml".to_string()], Vec::new());
    while let Some(file) = todo.pop() {
        if seen.contains(&file) {
            continue;
        }
        assert!(
            TAG_RUN.contains(&file.as_str()),
            "a tag runs {file}, not in TAG_RUN"
        );
        let text = fs::read_to_string(dir.join(&file)).unwrap();
        todo.extend(
            text.lines()
                .filter_map(|l| l.trim().strip_prefix("uses: ./.github/workflows/"))
                .map(str::to_string),
        );
        seen.push(file);
    }
    assert!(seen.iter().any(|f| f == "release-images.yml"), "{seen:?}");
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
