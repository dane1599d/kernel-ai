//! Spike S12-1 (ADR 0012): builds `cc.wasm`, xcc's `wcc` compiled by itself, from a
//! pinned upstream commit, and checks upstream's gen2-gen3 byte equality.
//!
//! cargo xtask ccwasm [--no-gen3]
//!
//! Writes `target/ccwasm/cc.wasm`, its SHA-256 and xcc's `include/` tree, and prints
//! the host tools the build used. Linux only (GitHub Actions): it needs `git`, `make`,
//! a host C compiler (`cc`), `llvm-ar` for wcc's libc, and for gen3 `node` and `npm`
//! (`tool/runwasi` runs gen2 on @wasmer/wasi and needs `commander`, a dev dependency).

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// xcc's main at the commit ADR 0012 read (2026-10-04), MIT.
pub const XCC_COMMIT: &str = "631e318432aeaaf72d6d2dfc93085766934314a2";
const XCC_URL: &str = "https://github.com/tyfkda/xcc.git";
/// The tools the build ran, each asked for its version.
const TOOLS: &[(&str, &str)] = &[
    ("git", "--version"),
    ("make", "--version"),
    ("cc", "--version"),
    ("llvm-ar", "--version"),
    ("node", "--version"),
    ("npm", "--version"),
];

pub fn main(args: &[String], target: &Path) -> Result<(), String> {
    let gen3 = match args {
        [] => true,
        [flag] if flag == "--no-gen3" => false,
        other => return Err(format!("ccwasm takes only --no-gen3, got {other:?}")),
    };
    let src = target.join("xcc").join(XCC_COMMIT);
    fetch(&src)?;
    run(&src, "make", &["-j4", "wcc-gen2"])?;
    if gen3 {
        run(&src, "npm", &["ci", "--ignore-scripts"])?;
        // Upstream's `wcc-gen3` passes compiler flags to tool/run-gen2wcc.sh without
        // a `--`, and the script refuses `-ansi` as its own option; the same make
        // rule with the `--` added.
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
        let gen2 = read(&src.join("cc.wasm"))?;
        let gen3 = read(&src.join("gen3cc.wasm"))?;
        if gen2 != gen3 {
            return Err("gen2 and gen3 cc.wasm differ".into());
        }
        println!("ccwasm: gen2 and gen3 cc.wasm are byte-identical");
    }
    let out = target.join("ccwasm");
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let wasm = read(&src.join("cc.wasm"))?;
    let sha = hex(&Sha256::digest(&wasm));
    fs::write(out.join("cc.wasm"), &wasm).map_err(|e| e.to_string())?;
    fs::write(out.join("cc.wasm.sha256"), format!("{sha}\n")).map_err(|e| e.to_string())?;
    copy_tree(&src.join("include"), &out.join("include"))?;
    println!("ccwasm: xcc {XCC_COMMIT}");
    println!("ccwasm: cc.wasm {} bytes, sha256 {sha}", wasm.len());
    for (tool, flag) in TOOLS {
        let version = Command::new(tool)
            .arg(flag)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(str::to_owned))
            .unwrap_or_else(|| "not found".into());
        println!("ccwasm: tool {tool}: {version}");
    }
    Ok(())
}

fn fetch(src: &Path) -> Result<(), String> {
    if src.join("Makefile").exists() {
        return Ok(());
    }
    fs::create_dir_all(src).map_err(|e| format!("{}: {e}", src.display()))?;
    run(src, "git", &["init", "-q"])?;
    run(
        src,
        "git",
        &["fetch", "-q", "--depth", "1", XCC_URL, XCC_COMMIT],
    )?;
    run(src, "git", &["checkout", "-q", "FETCH_HEAD"])
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    if !status.success() {
        return Err(format!("{program} {} failed: {status}", args.join(" ")));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path: PathBuf = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &dest)?;
        } else {
            fs::copy(&path, &dest).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
