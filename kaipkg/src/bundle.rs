//! `kai-pkg 1` (ADR 0011 §1): the `kai-source 1` grammar under its own header.
//! After the `kai-pkg 1` line come files, each `file <path> <len>`, a newline,
//! then exactly `len` bytes. Nothing is compressed or unpacked: the store runs
//! objects in place. At most [`MAX_FILES`] files and [`MAX_BYTES`] bytes; the
//! first file is `kai-package`, the header ([`crate::header`]).
//!
//! The reader is livecore's `source_files` (livecore/src/artifact.rs) with the
//! package limits; livecore keeps its own copy for `kai-source 1` until lane C
//! moves it here.

use alloc::vec::Vec;

/// The first line of a package file.
pub const MAGIC: &[u8] = b"kai-pkg 1\n";
pub const MAX_FILES: usize = 512;
pub const MAX_BYTES: usize = 64 << 20;
/// The header's path, the first file of every package.
pub const HEADER: &str = "kai-package";

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

/// The files of a `kai-pkg 1` package, in order. Each path is plain and appears
/// once, and the first is [`HEADER`].
pub fn files(package: &[u8]) -> Result<Vec<(&str, &[u8])>, &'static str> {
    if package.len() > MAX_BYTES {
        return Err("a package is over 64 MiB");
    }
    let mut rest = package
        .strip_prefix(MAGIC)
        .ok_or("not a kai-pkg 1 package")?;
    let mut files: Vec<(&str, &[u8])> = Vec::new();
    while !rest.is_empty() {
        if files.len() == MAX_FILES {
            return Err("a package has too many files");
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
        if len.starts_with('+') || (len.len() > 1 && len.starts_with('0')) {
            return Err("a file length is not a plain number");
        }
        let len: usize = len.parse().map_err(|_| "a file length is not a number")?;
        let body = &rest[newline + 1..];
        if len > body.len() {
            return Err("a file runs past the package");
        }
        valid_path(path)?;
        if files.iter().any(|(p, _)| *p == path) {
            return Err("a file appears twice in the package");
        }
        files.push((path, &body[..len]));
        rest = &body[len..];
    }
    match files.first() {
        Some((HEADER, _)) => Ok(files),
        _ => Err("a package's first file is not kai-package"),
    }
}

/// A package file of `files`, in order (for tests and xtask's repository tools).
pub fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::from(MAGIC);
    for (path, bytes) in files {
        out.extend_from_slice(alloc::format!("file {path} {}\n", bytes.len()).as_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_package_round_trips() {
        let bytes = build(&[(HEADER, b"kai-package 1\n"), ("app.wasm", b"\0asm")]);
        assert_eq!(
            files(&bytes).unwrap(),
            vec![
                (HEADER, &b"kai-package 1\n"[..]),
                ("app.wasm", &b"\0asm"[..])
            ]
        );
    }

    #[test]
    fn hostile_packages_are_refused() {
        let cases: &[&[u8]] = &[
            b"",
            b"kai-source 1\nfile kai-package 0\n",
            b"kai-pkg 1\nfile kai-package 9\nshort",
            b"kai-pkg 1\nfile kai-package +0\n",
            b"kai-pkg 1\nfile kai-package 00\n",
            b"kai-pkg 1\nfile kai-package -1\n",
            b"kai-pkg 1\nfile kai-package 0",
            b"kai-pkg 1\nfile ../kai-package 0\n",
            b"kai-pkg 1\nfile /kai-package 0\n",
            b"kai-pkg 1\nfile a 0\nfile kai-package 0\n",
            b"kai-pkg 1\nfile kai-package 0\nfile kai-package 0\n",
            b"kai-pkg 1\nfile kai-package 99999999999999999999999\n",
            b"kai-pkg 1\nfile kai\xffpackage 0\n",
            b"kai-pkg 1\n",
        ];
        for case in cases {
            assert!(files(case).is_err(), "{:?}", core::str::from_utf8(case));
        }
    }

    #[test]
    fn the_file_limit_holds() {
        let names: Vec<alloc::string::String> =
            (0..MAX_FILES).map(|i| alloc::format!("f{i}")).collect();
        let mut list: Vec<(&str, &[u8])> = vec![(HEADER, b"")];
        list.extend(names.iter().map(|n| (n.as_str(), &b""[..])));
        assert_eq!(files(&build(&list)), Err("a package has too many files"));
        list.pop();
        assert_eq!(files(&build(&list)).unwrap().len(), MAX_FILES);
    }
}
