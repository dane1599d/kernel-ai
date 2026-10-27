//! A version's artifact: a WebAssembly module with two custom sections, the
//! manifest (`kai.manifest`) and the bundled source (`kai.source`). The same bytes
//! ship as a boot module now and will be uploaded by the edit loop (M9b).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{MANIFEST_SECTION, MAX_ARTIFACT, SOURCE_SECTION};

const MAGIC: &[u8] = b"\0asm\x01\0\0\0";
/// Largest source bundle.
pub const MAX_SOURCE: usize = 256 * 1024;

/// Reads an unsigned LEB128 `u32` from `bytes` at `*at`.
fn leb128(bytes: &[u8], at: &mut usize) -> Result<u32, &'static str> {
    let mut value: u64 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*at).ok_or("a section is cut short")?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return u32::try_from(value).map_err(|_| "a length is too large");
        }
    }
    Err("a length is too long")
}

fn push_leb128(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The payload of the custom section called `name`, if there is exactly one.
pub fn custom_section<'a>(module: &'a [u8], name: &str) -> Result<Option<&'a [u8]>, &'static str> {
    if !module.starts_with(MAGIC) {
        return Err("not a WebAssembly module");
    }
    let mut at = MAGIC.len();
    let mut found = None;
    while at < module.len() {
        let id = module[at];
        at += 1;
        let size = leb128(module, &mut at)? as usize;
        let end = at
            .checked_add(size)
            .filter(|&end| end <= module.len())
            .ok_or("a section runs past the end")?;
        if id == 0 {
            let mut name_at = at;
            let name_len = leb128(&module[..end], &mut name_at)? as usize;
            let name_end = name_at
                .checked_add(name_len)
                .filter(|&e| e <= end)
                .ok_or("a section name runs past its section")?;
            if &module[name_at..name_end] == name.as_bytes() {
                if found.is_some() {
                    return Err("a custom section appears twice");
                }
                found = Some(&module[name_end..end]);
            }
        }
        at = end;
    }
    Ok(found)
}

/// Appends a custom section to a module.
pub fn append_custom_section(module: &mut Vec<u8>, name: &str, payload: &[u8]) {
    let mut body = Vec::with_capacity(5 + name.len() + payload.len());
    push_leb128(&mut body, name.len() as u32);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);
    module.push(0);
    push_leb128(module, body.len() as u32);
    module.extend_from_slice(&body);
}

pub use crate::manifest::{LIFECYCLES_LOADED, Manifest};

/// A source bundle: `kai-source 1`, then `file <path> <len>` and the bytes, per file.
pub fn bundle(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::from(&b"kai-source 1\n"[..]);
    for (path, bytes) in files {
        out.extend_from_slice(format!("file {path} {}\n", bytes.len()).as_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

/// Most files in a source bundle.
pub const MAX_SOURCE_FILES: usize = 256;

/// Whether `path` is a plain relative path: `/` between segments that are not
/// empty, `.` or `..`, with no backslash, colon, whitespace or control character.
pub fn valid_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.starts_with('/') {
        return Err("not a relative path");
    }
    if path.contains('\\') || path.contains(':') {
        return Err("backslashes and drive letters are not allowed");
    }
    if path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("spaces and control characters are not allowed");
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("empty, . and .. segments are not allowed");
    }
    Ok(())
}

/// The files in a source bundle, in order. Each path is plain (see
/// [`valid_path`]) and appears once.
pub fn source_files(bundle: &[u8]) -> Result<Vec<(&str, &[u8])>, &'static str> {
    let mut rest = bundle
        .strip_prefix(b"kai-source 1\n")
        .ok_or("not a kai-source 1 bundle")?;
    let mut files: Vec<(&str, &[u8])> = Vec::new();
    while !rest.is_empty() {
        if files.len() == MAX_SOURCE_FILES {
            return Err("a bundle has too many files");
        }
        let newline = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("a file header has no end")?;
        let header =
            core::str::from_utf8(&rest[..newline]).map_err(|_| "a file header is not text")?;
        let (path, len) = header
            .strip_prefix("file ")
            .and_then(|h| h.rsplit_once(' '))
            .ok_or("a file header is malformed")?;
        let len: usize = len.parse().map_err(|_| "a file length is not a number")?;
        let body = &rest[newline + 1..];
        if len > body.len() {
            return Err("a file runs past the bundle");
        }
        valid_path(path)?;
        if files.iter().any(|(p, _)| *p == path) {
            return Err("a file appears twice in the bundle");
        }
        files.push((path, &body[..len]));
        rest = &body[len..];
    }
    Ok(files)
}

/// A version's parts.
#[derive(Debug)]
pub struct Artifact<'a> {
    /// The whole module; the sections inside it are ignored when it runs.
    pub module: &'a [u8],
    pub manifest: Manifest,
    pub source: &'a [u8],
}

/// Reads an artifact's manifest and source, checking the source bundle parses.
pub fn parse(bytes: &[u8]) -> Result<Artifact<'_>, String> {
    if bytes.len() > MAX_ARTIFACT {
        return Err("the artifact is over 1 MiB".into());
    }
    let manifest = custom_section(bytes, MANIFEST_SECTION)?.ok_or("no kai.manifest section")?;
    let manifest = core::str::from_utf8(manifest).map_err(|_| "the manifest is not text")?;
    let manifest = Manifest::parse(manifest)?;
    let source = custom_section(bytes, SOURCE_SECTION)?.ok_or("no kai.source section")?;
    if source.len() > MAX_SOURCE {
        return Err("the source bundle is over 256 KiB".into());
    }
    source_files(source)?;
    Ok(Artifact {
        module: bytes,
        manifest,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn manifest() -> Manifest {
        crate::manifest::tests::manifest()
    }

    /// A module with one ordinary section, then the two custom sections.
    fn artifact() -> Vec<u8> {
        let mut module = Vec::from(MAGIC);
        module.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]);
        append_custom_section(
            &mut module,
            MANIFEST_SECTION,
            manifest().format().as_bytes(),
        );
        let source = bundle(&[("src/lib.rs", b"fn main() {}"), ("Cargo.toml", b"")]);
        append_custom_section(&mut module, SOURCE_SECTION, &source);
        module
    }

    #[test]
    fn an_artifact_round_trips() {
        let bytes = artifact();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.manifest, manifest());
        let files = source_files(parsed.source).unwrap();
        assert_eq!(
            files,
            vec![
                ("src/lib.rs", &b"fn main() {}"[..]),
                ("Cargo.toml", &b""[..])
            ]
        );
    }

    #[test]
    fn long_sections_use_multi_byte_lengths() {
        let mut module = Vec::from(MAGIC);
        let payload = vec![7u8; 300];
        append_custom_section(&mut module, "big", &payload);
        assert_eq!(custom_section(&module, "big"), Ok(Some(&payload[..])));
        assert_eq!(custom_section(&module, "other"), Ok(None));
    }

    #[test]
    fn malformed_modules_and_sections_are_refused() {
        assert!(custom_section(b"\x7fELF", "x").is_err());
        let mut cut = artifact();
        cut.truncate(cut.len() - 3);
        assert!(parse(&cut).is_err());
        let mut twice = artifact();
        append_custom_section(&mut twice, SOURCE_SECTION, b"kai-source 1\n");
        assert!(parse(&twice).unwrap_err().contains("twice"));
        let mut no_source = Vec::from(MAGIC);
        append_custom_section(
            &mut no_source,
            MANIFEST_SECTION,
            manifest().format().as_bytes(),
        );
        assert!(parse(&no_source).unwrap_err().contains("kai.source"));
    }

    #[test]
    fn a_bundle_that_lies_about_a_length_is_refused() {
        assert!(source_files(b"kai-source 1\nfile a.rs 99\nshort").is_err());
        assert!(source_files(b"kai-source 1\nfile 3\nabc").is_err());
        assert!(source_files(b"other").is_err());
        assert_eq!(source_files(b"kai-source 1\n").unwrap().len(), 0);
    }

    #[test]
    fn bundle_paths_are_plain_and_appear_once() {
        let bad_paths = [
            "/abs.rs",
            "a/../b.rs",
            "a//b.rs",
            "a\\b.rs",
            "c:x.rs",
            "a b.rs",
            "\u{c}page.rs",
        ];
        for bad in bad_paths {
            let one = bundle(&[(bad, b"")]);
            assert!(source_files(&one).is_err(), "{bad:?}");
        }
        let twice = bundle(&[("a.rs", b"1"), ("a.rs", b"2")]);
        assert_eq!(
            source_files(&twice),
            Err("a file appears twice in the bundle")
        );
        let many: Vec<String> = (0..=MAX_SOURCE_FILES).map(|n| format!("f{n}.rs")).collect();
        let parts: Vec<(&str, &[u8])> = many.iter().map(|p| (p.as_str(), &b""[..])).collect();
        assert!(source_files(&bundle(&parts)).is_err());
        assert!(valid_path("components/notes/src/lib.rs").is_ok());
    }
}
