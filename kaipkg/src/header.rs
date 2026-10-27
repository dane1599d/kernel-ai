//! `kai-package 1` (ADR 0011 §2): a package's header, its first file. Text, one
//! key per line, in a fixed order, strict like livecore's `Manifest::parse`:
//!
//! ```text
//! kai-package 1
//! name notes
//! version 2.1.0
//! kind app
//! summary Plain-text notes the AI can read and write
//! licence MPL-2.0
//! world kai:app@0.1
//! depends spell-en ^1.2
//! ```
//!
//! `world` is for `app` packages only, and required there; `blob <sha256>
//! <length>` is for `model` and `linux-layer` packages only, and required there.
//! `depends` lines are optional and repeatable. An unknown key, a key out of
//! order, a missing key or a repeated one is refused.

use crate::version::{Range, Version};
use crate::{Sha, unhex};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Package names a package may not take: every OS row group, the first word of
/// an `agentabi` OS row, so a package `fs` exporting `list` cannot make `fs_list`.
/// ADR 0011 has `agentabi` export this list; until it does, it lives here.
pub const RESERVED: &[&str] = &[
    "fs",
    "ui",
    "pkg",
    "vm",
    "app",
    "browser",
    "window",
    "media",
    "system",
    "device",
    "power",
    "update",
    "model",
    "linux",
    "net",
    "appearance",
];

/// The longest summary, in bytes.
pub const MAX_SUMMARY: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Data,
    Model,
    LinuxLayer,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Data => "data",
            Kind::Model => "model",
            Kind::LinuxLayer => "linux-layer",
        }
    }
}

/// A dependency: a package, or a `kai:*` WIT package the OS provides (§6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Depends {
    pub name: String,
    pub range: Range,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub version: Version,
    pub kind: Kind,
    /// Data, never instructions.
    pub summary: String,
    pub licence: String,
    /// `app` only: the `kai:*` WIT package and version the artifact is built
    /// against, as `kai:app@0.1`.
    pub world: Option<String>,
    pub depends: Vec<Depends>,
    /// `model` and `linux-layer` only: the payload's SHA-256 and length.
    pub blob: Option<(Sha, u64)>,
}

/// Whether `name` may name a package: `[a-z][a-z0-9-]{1,11}`, not reserved.
pub fn valid_name(name: &str) -> Result<(), &'static str> {
    let bytes = name.as_bytes();
    let shaped = (2..=12).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if !shaped {
        return Err("a package name is not 2-12 of a-z, 0-9 and -, starting with a letter");
    }
    if RESERVED.contains(&name) {
        return Err("a package name is an OS row group");
    }
    Ok(())
}

/// A `kai:<name>@<major>.<minor>` world.
fn valid_world(world: &str) -> bool {
    let Some((package, version)) = world.split_once('@') else {
        return false;
    };
    let package_ok = package
        .strip_prefix("kai:")
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase()));
    let version_ok = version.split_once('.').is_some_and(|(a, b)| {
        [a, b]
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    package_ok && version_ok
}

/// An SPDX expression's shape: identifiers, `AND`, `OR`, `WITH` and parentheses.
fn valid_licence(licence: &str) -> bool {
    !licence.is_empty()
        && licence.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'+' | b' ' | b'(' | b')')
        })
}

impl Header {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text
            .strip_suffix('\n')
            .ok_or("a header ends without a newline")?
            .split('\n');
        if lines.next() != Some("kai-package 1") {
            return Err("not a kai-package 1 header".into());
        }
        let mut field = |key: &str| -> Result<String, String> {
            let line = lines
                .next()
                .ok_or_else(|| alloc::format!("no {key} line"))?;
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| alloc::format!("expected the {key} line, got {line:?}"))
        };
        let name = field("name")?;
        valid_name(&name)?;
        let version = Version::parse(&field("version")?)?;
        let kind = match field("kind")?.as_str() {
            "app" => Kind::App,
            "data" => Kind::Data,
            "model" => Kind::Model,
            "linux-layer" => Kind::LinuxLayer,
            "tool" => return Err("kind tool is reserved in format 1".into()),
            other => return Err(alloc::format!("unknown kind {other:?}")),
        };
        let summary = field("summary")?;
        if summary.len() > MAX_SUMMARY || summary.chars().any(char::is_control) {
            return Err("a summary is over 80 bytes or holds a control character".into());
        }
        let licence = field("licence")?;
        if !valid_licence(&licence) {
            return Err("a licence is not an SPDX expression".into());
        }
        let mut header = Header {
            name,
            version,
            kind,
            summary,
            licence,
            world: None,
            depends: Vec::new(),
            blob: None,
        };
        let rest: Vec<&str> = lines.collect();
        let mut rest = rest.as_slice();
        if kind == Kind::App {
            let (line, more) = rest.split_first().ok_or("an app has no world line")?;
            let world = line.strip_prefix("world ").filter(|w| valid_world(w));
            header.world = Some(
                world
                    .ok_or("an app's second key is not a valid world")?
                    .into(),
            );
            rest = more;
        }
        while let Some(dep) = rest.first().and_then(|l| l.strip_prefix("depends ")) {
            let (name, range) = dep.split_once(' ').ok_or("a depends line has no range")?;
            if !name.starts_with("kai:") {
                valid_name(name)?;
            }
            let range = Range::parse(range)?;
            header.depends.push(Depends {
                name: name.into(),
                range,
            });
            rest = &rest[1..];
        }
        if matches!(kind, Kind::Model | Kind::LinuxLayer) {
            let (line, more) = rest.split_first().ok_or("a blob kind has no blob line")?;
            let words: Vec<&str> = line.split(' ').collect();
            let blob = match words.as_slice() {
                ["blob", sha, len] => unhex(sha).zip(len.parse::<u64>().ok()),
                _ => None,
            };
            header.blob = Some(blob.ok_or("a blob line is not blob <sha256> <length>")?);
            rest = more;
        }
        match rest.first() {
            None => Ok(header),
            Some(line) => Err(alloc::format!("an unknown or misplaced line {line:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTES: &str = "kai-package 1\nname notes\nversion 2.1.0\nkind app\n\
        summary Plain-text notes the AI can read and write\nlicence MPL-2.0\n\
        world kai:app@0.1\ndepends spell-en ^1.2\ndepends kai:app ^0.1\n";

    #[test]
    fn the_adrs_header_parses() {
        let h = Header::parse(NOTES).unwrap();
        assert_eq!(h.name, "notes");
        assert_eq!(h.version, Version::new(2, 1, 0));
        assert_eq!(h.world.as_deref(), Some("kai:app@0.1"));
        assert_eq!(h.depends.len(), 2);
        assert!(h.depends[0].range.matches(&Version::new(1, 9, 0)));
    }

    #[test]
    fn a_model_header_carries_its_blob() {
        let sha = crate::hex(&crate::sha256(b"weights"));
        let text = alloc::format!(
            "kai-package 1\nname qwen-4b\nversion 1.0.0\nkind model\nsummary A model\n\
             licence Apache-2.0\nblob {sha} 2497281120\n"
        );
        let h = Header::parse(&text).unwrap();
        assert_eq!(h.blob, Some((crate::sha256(b"weights"), 2_497_281_120)));
        assert_eq!(h.world, None);
    }

    #[test]
    fn names_follow_the_rule() {
        for good in ["notes", "spell-en", "a1", "abcdefghijkl"] {
            assert_eq!(valid_name(good), Ok(()), "{good}");
        }
        for bad in [
            "n",
            "Notes",
            "1abc",
            "-ab",
            "abcdefghijklm",
            "fs",
            "pkg",
            "no_tes",
            "nö",
        ] {
            assert!(valid_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hostile_headers_are_refused() {
        let swap = |from: &str, to: &str| NOTES.replacen(from, to, 1);
        let cases = [
            String::new(),
            NOTES.trim_end().into(),
            swap("kai-package 1", "kai-package 2"),
            swap("name notes\nversion 2.1.0", "version 2.1.0\nname notes"),
            swap("kind app", "kind tool"),
            swap("kind app", "kind exe"),
            swap("world kai:app@0.1\n", ""),
            swap("world kai:app@0.1", "world app@0.1"),
            swap("licence MPL-2.0", "licence MPL/2.0"),
            swap("summary Plain", "summary Plain\u{7}"),
            swap("depends spell-en ^1.2", "depends spell-en"),
            swap("depends spell-en ^1.2", "depends fs ^1.2"),
            alloc::format!("{NOTES}abi 1.0\n"),
            alloc::format!("{NOTES}world kai:app@0.1\n"),
            swap("kind app\n", "kind data\n"),
            swap("name notes", "name notes "),
            swap("version 2.1.0", "version 2.1"),
            alloc::format!("{NOTES}\n"),
            swap(
                "summary Plain-text",
                &alloc::format!("summary {}", "x".repeat(81)),
            ),
        ];
        for case in &cases {
            assert!(Header::parse(case).is_err(), "{case:?}");
        }
    }
}
