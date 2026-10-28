//! Every tracked file is covered (ADR 0009 D5, as widened 2026-10-27): by its own
//! `SPDX-License-Identifier:` header, by a path annotation in `REUSE.toml`, or
//! both, and then the two must agree. The licence texts themselves, `LICENSES/`
//! and `LICENSE`, need neither. Headers stay optional.
//!
//! `REUSE.toml` is read in the shape we write it: `[[annotations]]` blocks whose
//! `path` is a string or a one-line array of globs (`*` within a path part, `**`
//! across them) and whose `SPDX-License-Identifier` is one string. As REUSE 3.3
//! says, the last annotation that matches a file is the one that applies.

/// One `[[annotations]]` block: its globs and its licence expression.
#[derive(Debug, PartialEq)]
pub struct Annotation {
    pub paths: Vec<String>,
    pub expression: String,
}

/// The annotations of a `REUSE.toml`, in order.
pub fn parse(text: &str) -> Result<Vec<Annotation>, String> {
    let mut out: Vec<Annotation> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[annotations]]" {
            out.push(Annotation {
                paths: Vec::new(),
                expression: String::new(),
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("REUSE.toml:{}: not `key = value`", n + 1));
        };
        let (key, value) = (key.trim(), value.trim());
        let Some(block) = out.last_mut() else {
            continue; // `version`, before any annotation
        };
        // Only the keys the check reads; `precedence` and the copyright lines
        // are REUSE's, not ours.
        if !matches!(key, "path" | "SPDX-License-Identifier") {
            continue;
        }
        let strings = strings(value).ok_or_else(|| {
            format!(
                "REUSE.toml:{}: {key} is not a string or a list of them",
                n + 1
            )
        })?;
        match key {
            "path" => block.paths = strings,
            "SPDX-License-Identifier" => match strings.as_slice() {
                [one] => block.expression = one.clone(),
                _ => return Err(format!("REUSE.toml:{}: one licence expression", n + 1)),
            },
            _ => {}
        }
    }
    match out
        .iter()
        .position(|a| a.paths.is_empty() || a.expression.is_empty())
    {
        Some(i) => Err(format!(
            "REUSE.toml: annotation {} has no path or no licence",
            i + 1
        )),
        None => Ok(out),
    }
}

/// `"a"` or `["a", "b"]`; a comma inside quotes is the string's.
fn strings(value: &str) -> Option<Vec<String>> {
    let inner = match value.strip_prefix('[') {
        Some(rest) => rest.strip_suffix(']')?,
        None => value,
    };
    // Between quotes: the strings; outside them: nothing but commas and spaces.
    let parts: Vec<&str> = inner.split('"').collect();
    let (outside, quoted): (Vec<_>, Vec<_>) =
        parts.iter().enumerate().partition(|(i, _)| i % 2 == 0);
    let separated = outside.iter().enumerate().all(|(i, (_, s))| {
        let s = s.trim();
        match i == 0 || i == outside.len() - 1 {
            true => s.is_empty(),
            false => s == ",",
        }
    });
    (parts.len() % 2 == 1 && !quoted.is_empty() && separated)
        .then(|| quoted.iter().map(|(_, s)| s.to_string()).collect())
}

/// Whether `path` (`/`-separated) matches `glob`.
pub fn matches(glob: &str, path: &str) -> bool {
    let glob: Vec<&str> = glob.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    parts(&glob, &path)
}

fn parts(glob: &[&str], path: &[&str]) -> bool {
    match (glob.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            parts(&glob[1..], path) || (!path.is_empty() && parts(glob, &path[1..]))
        }
        (Some(g), Some(p)) => part(g, p) && parts(&glob[1..], &path[1..]),
        _ => false,
    }
}

/// One path part against one glob part, where `*` is any run of characters.
fn part(glob: &str, text: &str) -> bool {
    match glob.split_once('*') {
        None => glob == text,
        Some((head, rest)) => {
            let Some(text) = text.strip_prefix(head) else {
                return false;
            };
            (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .any(|i| part(rest, &text[i..]))
        }
    }
}

/// The expression the last matching annotation gives `path`.
pub fn licence_of<'a>(annotations: &'a [Annotation], path: &str) -> Option<&'a str> {
    annotations
        .iter()
        .rev()
        .find(|a| a.paths.iter().any(|g| matches(g, path)))
        .map(|a| a.expression.as_str())
}

/// What is wrong with the tree's coverage: each tracked file with its header's
/// expression, if it has one.
pub fn problems(files: &[(String, Option<String>)], annotations: &[Annotation]) -> Vec<String> {
    let mut out = Vec::new();
    for (path, header) in files {
        if path == "LICENSE" || path.starts_with("LICENSES/") {
            continue;
        }
        match (header.as_deref(), licence_of(annotations, path)) {
            (None, None) => out.push(format!("{path}: no SPDX header and no REUSE.toml path")),
            (Some(h), Some(r)) if !same(h, r) => out.push(format!(
                "{path}: its header says `{h}` but REUSE.toml says `{r}`"
            )),
            _ => {}
        }
    }
    out
}

/// Two expressions with the same words, however spaced.
fn same(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"version = 1

[[annotations]]
path = "**"
SPDX-License-Identifier = "MPL-2.0"

# Compiled into other programs.
[[annotations]]
path = ["kabi/**", "wit/*.wit"]
SPDX-FileCopyrightText = ["a", "b"]
SPDX-License-Identifier = "MIT OR Apache-2.0"
"#;

    #[test]
    fn the_last_matching_annotation_applies() {
        let a = parse(SAMPLE).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(licence_of(&a, "kernel/src/main.rs"), Some("MPL-2.0"));
        assert_eq!(licence_of(&a, "kabi/src/lib.rs"), Some("MIT OR Apache-2.0"));
        assert_eq!(licence_of(&a, "wit/view.wit"), Some("MIT OR Apache-2.0"));
        assert_eq!(licence_of(&a, "wit/frozen/view.wit"), Some("MPL-2.0"));
        assert_eq!(licence_of(&[], "README.md"), None);
    }

    #[test]
    fn globs_match_path_parts() {
        assert!(matches("**", "a/b/c.rs"));
        assert!(matches("a/**", "a/b/c.rs"));
        assert!(matches("a/**/c.rs", "a/c.rs"));
        assert!(!matches("a/**", "ab/c.rs"));
        assert!(matches("*.toml", "Cargo.toml"));
        assert!(!matches("*.toml", "x/Cargo.toml"));
        assert!(matches("kai-*@0.1.0.wit", "kai-ui@0.1.0.wit"));
    }

    #[test]
    fn a_file_with_neither_or_with_two_licences_is_a_problem() {
        let a = parse(&SAMPLE.replace("path = \"**\"", "path = \"src/**\"")).unwrap();
        let files = [
            (String::from("src/a.rs"), None),
            (String::from("src/b.rs"), Some(String::from("MPL-2.0"))),
            (
                String::from("kabi/c.rs"),
                Some(String::from("MIT  OR Apache-2.0")),
            ),
            (String::from("kabi/d.rs"), Some(String::from("MPL-2.0"))),
            (String::from("README.md"), None),
            (String::from("LICENSES/MIT.txt"), None),
        ];
        assert_eq!(
            problems(&files, &a),
            [
                "kabi/d.rs: its header says `MPL-2.0` but REUSE.toml says `MIT OR Apache-2.0`",
                "README.md: no SPDX header and no REUSE.toml path",
            ]
        );
    }

    #[test]
    fn a_reuse_toml_out_of_shape_is_refused() {
        assert!(parse("[[annotations]]\npath = \"**\"\n").is_err());
        assert!(
            parse(
                "[[annotations]]\nSPDX-License-Identifier = [\"MIT\", \"0BSD\"]\npath = \"**\"\n"
            )
            .is_err()
        );
        assert!(parse("[[annotations]]\npath = **\n").is_err());
    }

    #[test]
    fn the_trees_reuse_toml_reads() {
        let text = std::fs::read_to_string(crate::workspace_root().join("REUSE.toml")).unwrap();
        assert!(!parse(&text).unwrap().is_empty());
    }
}
