//! A lint over README.md's "Try it" section (G65): every `cargo xtask <sub>`
//! it types names a subcommand xtask has, every `--flag` it shows (in a
//! command or in prose) is one xtask reads, and every repo path it links or
//! names exists. The front page cannot drift from the tooling between
//! releases.
//!
//! A subcommand is a string literal in main.rs's dispatch (or in options.rs,
//! which parses the build and boot tasks main.rs falls through to), and a
//! flag typed after it is a string literal in that subcommand's module (the
//! `mod::main` its dispatch line calls; a module that cannot be found falls
//! back to all of xtask/src). A flag in prose is checked against all of
//! xtask/src, except [`FOREIGN`] ones. The section is read as text, not
//! Markdown: fenced blocks, inline code spans and link targets.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The lines of the `## Try it` section, up to the next `## ` heading.
fn section(readme: &str) -> Vec<&str> {
    let mut lines = readme.lines().skip_while(|l| l.trim() != "## Try it");
    lines.next();
    lines.take_while(|l| !l.starts_with("## ")).collect()
}

/// The command lines typed in fenced blocks, and the inline code spans that
/// start `cargo xtask`, each with a `# comment` cut off.
fn commands(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            if line.trim_start().starts_with("cargo xtask ") {
                out.push(line.split(" #").next().unwrap_or(line).trim().to_string());
            }
        } else {
            out.extend(spans(line).filter(|s| s.starts_with("cargo xtask ")));
        }
    }
    out
}

/// Extensions that make a plain code span a file name.
const FILES: &[&str] = &["toml", "json", "md", "yml", "yaml", "rs", "lock", "sh"];

/// Flags the section shows that belong to another tool than xtask.
const FOREIGN: &[&str] = &[];

/// A link's target: `<a b>` up to the `>`, otherwise up to the first space,
/// which starts a `"title"`.
fn link_target(inside: &str) -> &str {
    let inside = inside.trim();
    match inside.strip_prefix('<') {
        Some(rest) => rest.split('>').next().unwrap_or(rest),
        None => inside.split_whitespace().next().unwrap_or(inside),
    }
}

/// The inline code spans of one line.
fn spans(line: &str) -> impl Iterator<Item = String> + '_ {
    line.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.trim().to_string())
}

/// The `--flag` words of a piece of text, without a `=value`.
fn flags(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|w| w.starts_with("--") && w.len() > 2)
        .map(|w| w.split('=').next().unwrap_or(w).to_string())
        .collect()
}

/// The repo paths the section names: link targets that are not URLs or
/// anchors, and inline code spans that are a plain path with a `/`.
fn paths(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        let mut rest = *line;
        while let Some(at) = rest.find("](") {
            let target = &rest[at + 2..];
            let end = target.find(')').unwrap_or(target.len());
            let link = link_target(&target[..end]);
            if !link.contains("://") && !link.starts_with('#') && !link.starts_with("mailto:") {
                out.push(link.split('#').next().unwrap_or(link).to_string());
            }
            rest = &target[end..];
        }
        for span in spans(line) {
            let plain = span
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b));
            // A path with a slash, or a plain file of the repo's top level
            // (rust-toolchain.toml) by its extension.
            let file = span
                .rsplit_once('.')
                .is_some_and(|(_, ext)| FILES.contains(&ext));
            let slashed = span.contains('/') && !span.starts_with('/') && !span.ends_with('/');
            if plain && (slashed || file) {
                out.push(span);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// xtask's source text: main.rs, options.rs, each module by name, and all.
struct Source {
    main: String,
    options: String,
    modules: Vec<(String, String)>,
    all: String,
}

impl Source {
    /// The text of module `name` (`name.rs` and `name/**`), or all of it when
    /// there is no such module.
    fn module(&self, name: &str) -> &str {
        self.modules
            .iter()
            .find(|(n, _)| n == name)
            .map_or(&self.all, |(_, text)| text)
    }

    /// The module the dispatch calls for `sub`, whose `Some("a" | "b") =>
    /// module::main(..)` line names it; None when main.rs does not dispatch
    /// it (the parser in options.rs takes it).
    fn dispatched(&self, sub: &str) -> Option<&str> {
        let quoted = format!("\"{sub}\"");
        let line = self
            .main
            .lines()
            .find(|l| l.contains("Some(") && l.contains(&quoted) && l.contains("=>"))?;
        let call = line.split("=>").nth(1)?.trim();
        Some(call.split("::").next().unwrap_or(call).trim())
    }
}

fn read_rs(dir: &Path, out: &mut String) {
    for entry in fs::read_dir(dir).expect("xtask/src").flatten() {
        let path = entry.path();
        if path.is_dir() {
            read_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "readme_lint.rs")
        {
            out.push_str(&fs::read_to_string(&path).expect("a source file"));
        }
    }
}

fn sources() -> Source {
    let src = root().join("xtask/src");
    let (mut all, mut modules) = (String::new(), Vec::new());
    for entry in fs::read_dir(&src).expect("xtask/src").flatten() {
        let path = entry.path();
        let name = path.file_stem().map(|n| n.to_string_lossy().to_string());
        let mut text = String::new();
        if path.is_dir() {
            read_rs(&path, &mut text);
        } else if path.extension().is_some_and(|e| e == "rs")
            && name.as_deref() != Some("readme_lint")
        {
            text = fs::read_to_string(&path).expect("a source file");
        }
        if let (Some(name), false) = (name, text.is_empty()) {
            // A module's file and its directory (`vbox.rs`, `vbox/`) join.
            match modules
                .iter_mut()
                .find(|(n, _): &&mut (String, String)| *n == name)
            {
                Some((_, joined)) => joined.push_str(&text),
                None => modules.push((name, text.clone())),
            }
            all.push_str(&text);
        }
    }
    let text = |name: &str| fs::read_to_string(src.join(name)).expect("an xtask source");
    Source {
        main: text("main.rs"),
        options: text("options.rs"),
        modules,
        all,
    }
}

/// What the README's `lines` get wrong against `source` (xtask's text) and the
/// repo at `repo`: one line each, and how many commands, flags and paths it
/// checked.
fn lint(lines: &[&str], source: &Source, repo: &Path) -> (Vec<String>, [usize; 3]) {
    let (mut errors, mut seen) = (Vec::new(), [0; 3]);
    let literal = |text: &str, word: &str| text.contains(&format!("\"{word}\""));
    let mut shown = Vec::new();
    for command in commands(lines) {
        seen[0] += 1;
        let mut words = command.split_whitespace().skip(2);
        let sub = words.next().unwrap_or("");
        // The text a flag after `sub` must be a literal in.
        let scope = match source.dispatched(sub) {
            Some(module) => source.module(module),
            None if literal(&source.options, sub) => &source.options,
            None => {
                errors.push(format!("`{command}`: xtask has no `{sub}` subcommand"));
                continue;
            }
        };
        for flag in flags(&command) {
            seen[1] += 1;
            if !literal(scope, &flag) {
                errors.push(format!("`{command}`: `{sub}` does not read `{flag}`"));
            }
        }
    }
    for line in lines {
        for span in spans(line) {
            if !span.starts_with("cargo ") {
                shown.extend(flags(&span));
            }
        }
    }
    shown.sort();
    shown.dedup();
    for flag in shown {
        if FOREIGN.contains(&flag.as_str()) {
            continue;
        }
        seen[1] += 1;
        if !literal(&source.all, &flag) {
            errors.push(format!("`{flag}`: no xtask parser reads this flag"));
        }
    }
    for path in paths(lines) {
        seen[2] += 1;
        if !repo.join(&path).exists() {
            errors.push(format!("`{path}`: the README names a path the repo lacks"));
        }
    }
    (errors, seen)
}

#[test]
fn the_readmes_try_it_matches_the_tooling() {
    let readme = fs::read_to_string(root().join("README.md")).expect("README.md");
    let lines = section(&readme);
    assert!(
        lines.len() > 20,
        "the Try it section: {} lines",
        lines.len()
    );
    let (errors, seen) = lint(&lines, &sources(), &root());
    // It reads the commands, flags and paths: today's README has several of each.
    assert!(seen[0] >= 4 && seen[1] >= 4 && seen[2] >= 1, "{seen:?}");
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn the_lint_refuses_a_renamed_flag_a_missing_subcommand_and_a_missing_path() {
    let readme = "\
## Try it

```bash
cargo xtask vbox --start    # boots it
cargo xtask vbox --begin
cargo xtask doctor --start
cargo xtask vboxes
```

Run `cargo xtask doctor`; `--name NAME` and `--rename NAME` pick a VM. See
[the models](xtask/models.json), [titled](xtask/models.json \"the pins\"),
[angled](<xtask/models.json>) and [gone](xtask/nope.json), or `xtask/src/main.rs`
and `xtask/src/none.rs`; `rust-toolchain.toml` and `missing.toml` are top-level
files. A [link](https://example.com/x/y) and [here](#anchor) do not count.

## Next
`cargo xtask elsewhere --unseen`
";
    let source = Source {
        main: "Some(\"vbox\") => vbox::main(&a),\nSome(\"doctor\") => doctor::main(&a),".into(),
        options: "\"run\"".into(),
        modules: vec![
            ("vbox".into(), "\"--start\"".into()),
            ("doctor".into(), "\"--check\"".into()),
        ],
        all: "\"--start\" \"--check\" \"--name\"".into(),
    };
    let lines = section(readme);
    let (errors, seen) = lint(&lines, &source, &root());
    let has = |what: &str| errors.iter().filter(|e| e.contains(what)).count();
    assert_eq!(errors.len(), 7, "{errors:#?}");
    assert_eq!(has("`vboxes`"), 1, "{errors:#?}");
    // A flag that is another subcommand's is refused, and so is a renamed one.
    assert_eq!(has("`doctor` does not read `--start`"), 1, "{errors:#?}");
    assert_eq!(has("`vbox` does not read `--begin`"), 1, "{errors:#?}");
    assert_eq!(has("--rename"), 1, "{errors:#?}");
    // A link with a title or in angle brackets is read; so is a top-level file.
    assert_eq!(has("xtask/nope.json"), 1, "{errors:#?}");
    assert_eq!(has("xtask/src/none.rs"), 1, "{errors:#?}");
    assert_eq!(has("`missing.toml`"), 1, "{errors:#?}");
    // The `## Next` section is not read.
    assert_eq!(seen, [5, 5, 6]);
}
