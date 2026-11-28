//! A lint over README.md's "Try it" section (G65): every `cargo xtask <sub>`
//! it types names a subcommand xtask has, every `--flag` it shows (in a
//! command or in prose) is one xtask reads, and every repo path it links or
//! names exists. The front page cannot drift from the tooling between
//! releases.
//!
//! Subcommands and flags are looked up as string literals in `xtask/src`,
//! which is how both the dispatch in main.rs and every parser spell them, so a
//! renamed flag is no longer found. The section is read as text, not
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
            let link = &target[..end];
            if !link.contains("://") && !link.starts_with('#') && !link.starts_with("mailto:") {
                out.push(link.split('#').next().unwrap_or(link).to_string());
            }
            rest = &target[end..];
        }
        for span in spans(line) {
            let plain = span
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b));
            if plain && span.contains('/') && !span.starts_with('/') && !span.ends_with('/') {
                out.push(span);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every `.rs` file's text under `xtask/src`, but this one's.
fn sources() -> String {
    fn walk(dir: &Path, out: &mut String) {
        for entry in fs::read_dir(dir).expect("xtask/src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "readme_lint.rs")
            {
                out.push_str(&fs::read_to_string(&path).expect("a source file"));
            }
        }
    }
    let mut out = String::new();
    walk(&root().join("xtask/src"), &mut out);
    out
}

/// What the README's `lines` get wrong against `source` (xtask's text) and the
/// repo at `repo`: one line each, and how many commands, flags and paths it
/// checked.
fn lint(lines: &[&str], source: &str, repo: &Path) -> (Vec<String>, [usize; 3]) {
    let (mut errors, mut seen) = (Vec::new(), [0; 3]);
    let literal = |word: &str| source.contains(&format!("\"{word}\""));
    let mut shown = Vec::new();
    for command in commands(lines) {
        seen[0] += 1;
        let mut words = command.split_whitespace().skip(2);
        let sub = words.next().unwrap_or("");
        if !literal(sub) {
            errors.push(format!("`{command}`: xtask has no `{sub}` subcommand"));
        }
        shown.extend(flags(&command));
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
        seen[1] += 1;
        if !literal(&flag) {
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
cargo xtask vboxes
```

Run `cargo xtask doctor`; `--name NAME` and `--rename NAME` pick a VM. See
[the models](xtask/models.json) and [gone](xtask/nope.json), or `xtask/src/main.rs`
and `xtask/src/none.rs`. A [link](https://example.com/x/y) and [here](#anchor) do not count.

## Next
`cargo xtask elsewhere --unseen`
";
    let source = "\"vbox\" \"doctor\" \"--start\" \"--name\"";
    let lines = section(readme);
    let (errors, seen) = lint(&lines, source, &root());
    assert_eq!(errors.len(), 5, "{errors:#?}");
    assert!(errors[0].contains("`vboxes`"), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("--begin")));
    assert!(errors.iter().any(|e| e.contains("--rename")));
    assert!(errors.iter().any(|e| e.contains("xtask/nope.json")));
    assert!(errors.iter().any(|e| e.contains("xtask/src/none.rs")));
    // The `## Next` section is not read.
    assert_eq!(seen, [4, 4, 4]);
}
