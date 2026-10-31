//! `cargo xtask ccwasm [--check] [--no-gen3]` (ADR 0012 D2): builds `cc.wasm`,
//! xcc's `wcc` compiled by itself, from the commit `third_party/xcc/UPSTREAM.toml`
//! pins, and writes it with the headers sessions may include into
//! `third_party/xcc/`. `--check` writes nothing and fails if the build differs
//! from what is there (its SHA-256 and every header byte for byte).
//!
//! Linux or WSL2 only: it needs `git`, `make`, a host C compiler (`cc`) and
//! `llvm-ar` for wcc's libc, and for gen3 also `node` and `npm`
//! (`tool/runwasi` runs gen2 on @wasmer/wasi and needs `commander`, a dev
//! dependency). Upstream's gen2-gen3 check (`wcc-diff-gen23`) runs as a
//! section-by-section comparison: at the pinned commit only the `name` custom
//! section differs (S12-1).

pub mod bind;

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;

const XCC_URL: &str = "https://github.com/tyfkda/xcc.git";
/// The headers sessions may include; ccserve's `XCC_HEADERS` embeds the same.
const HEADERS: [&str; 9] = [
    "float.h",
    "inttypes.h",
    "limits.h",
    "stdarg.h",
    "stdbool.h",
    "stddef.h",
    "stdint.h",
    "stdnoreturn.h",
    "string.h",
];

pub fn main(args: &[String], root: &Path) -> Result<(), String> {
    let (mut check, mut gen3) = (false, true);
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--no-gen3" => gen3 = false,
            other => return Err(format!("ccwasm takes --check and --no-gen3, not {other}")),
        }
    }
    let pinned = root.join("third_party").join("xcc");
    let commit = upstream_commit(&pinned.join("UPSTREAM.toml"))?;
    let src = root.join("target").join("xcc").join(&commit);
    fetch(&src, &commit)?;
    run(&src, "make", &["-j4", "wcc-gen2"])?;
    let wasm = read(&src.join("cc.wasm"))?;
    if gen3 {
        run(&src, "npm", &["ci", "--ignore-scripts"])?;
        // Upstream's `wcc-gen3` passes compiler flags to tool/run-gen2wcc.sh
        // without a `--`, and the script refuses `-ansi` as its own option: the
        // same make rule with the `--` added.
        run(
            &src,
            "make",
            &[
                "-j4",
                "HOST_TARGET=gen2",
                "HOST_WCC=./tool/run-gen2wcc.sh --",
                "CC=./tool/run-gen2wcc.sh --",
                "WCC_TARGET=gen3",
                "wcc-self-hosting",
            ],
        )?;
        same_code(&wasm, &read(&src.join("gen3cc.wasm"))?)?;
    }
    let sha = hex(&Sha256::digest(&wasm));
    println!(
        "ccwasm: xcc {commit}: cc.wasm {} bytes, sha256 {sha}",
        wasm.len()
    );
    let mut files = vec![
        ("cc.wasm".to_string(), wasm),
        (
            "cc.wasm.sha256".to_string(),
            format!("{sha}  cc.wasm\n").into_bytes(),
        ),
    ];
    for header in HEADERS {
        let bytes = read(&src.join("include").join(header))?;
        files.push((format!("include/{header}"), bytes));
    }
    for (name, bytes) in files {
        let path = pinned.join(&name);
        if check {
            if read(&path)? != bytes {
                return Err(format!("{} differs from the build", path.display()));
            }
        } else {
            fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    if check {
        println!("ccwasm: third_party/xcc matches the build");
    }
    Ok(())
}

/// `commit = "…"` in UPSTREAM.toml: 40 hex digits.
fn upstream_commit(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .find_map(|line| line.strip_prefix("commit = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| format!("{} pins no commit", path.display()))
}

/// gen2 and gen3 must agree in every section but the custom ones.
fn same_code(gen2: &[u8], gen3: &[u8]) -> Result<(), String> {
    if sections(gen2)? != sections(gen3)? {
        return Err("gen2 and gen3 cc.wasm differ outside custom sections".into());
    }
    println!("ccwasm: gen2 and gen3 agree outside custom sections");
    Ok(())
}

/// A module's non-custom sections, as (id, payload).
fn sections(wasm: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut out = Vec::new();
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let byte = *wasm.get(at).ok_or("truncated section size")?;
            at += 1;
            size |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 || shift > 28 {
                break;
            }
        }
        let end = at.checked_add(size).filter(|&e| e <= wasm.len());
        let payload = &wasm[at..end.ok_or("truncated section")?];
        if id != 0 {
            out.push((id, payload));
        }
        at += size;
    }
    Ok(out)
}

fn fetch(src: &Path, commit: &str) -> Result<(), String> {
    if src.join("Makefile").exists() {
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(src)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        return match String::from_utf8_lossy(&head.stdout).trim() == commit {
            true => Ok(()),
            false => Err(format!(
                "{} is not at {commit}; delete it to fetch again",
                src.display()
            )),
        };
    }
    fs::create_dir_all(src).map_err(|e| format!("{}: {e}", src.display()))?;
    run(src, "git", &["init", "-q"])?;
    run(
        src,
        "git",
        &["fetch", "-q", "--depth", "1", XCC_URL, commit],
    )?;
    run(src, "git", &["checkout", "-q", "FETCH_HEAD"])
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("{program} {} failed: {status}", args.join(" "))),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_commit_reads() {
        let root = crate::workspace_root();
        let commit = upstream_commit(&root.join("third_party/xcc/UPSTREAM.toml")).unwrap();
        assert_eq!(commit, "631e318432aeaaf72d6d2dfc93085766934314a2");
    }

    #[test]
    fn sections_skip_custom_ones() {
        // Magic, version, a type section of 1 byte, a custom section of 2.
        let wasm = b"\0asm\x01\0\0\0\x01\x01\x00\x00\x02\x01x";
        assert_eq!(sections(wasm).unwrap(), [(1u8, &b"\x00"[..])]);
        assert!(sections(b"\0asm\x01\0\0\0\x01\x05\x00").is_err());
    }

    #[test]
    fn the_pinned_files_are_there() {
        let pinned = crate::workspace_root().join("third_party/xcc");
        let sha = fs::read_to_string(pinned.join("cc.wasm.sha256")).unwrap();
        let wasm = read(&pinned.join("cc.wasm")).unwrap();
        let want = hex(&Sha256::digest(&wasm));
        assert_eq!(sha.split_whitespace().next(), Some(want.as_str()));
        for header in HEADERS {
            assert!(pinned.join("include").join(header).exists(), "{header}");
        }
    }
}
