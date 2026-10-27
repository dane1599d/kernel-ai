//! `cargo xtask fs`: kaifs data disks on the host (ADR 0004 §5-8; P2 task 7).
//! Plain Rust over an image file, so it runs on Windows without WSL.
//!
//! ```text
//! cargo xtask fs mkfs IMAGE [--size MIB] [--label TEXT] [--force]
//! cargo xtask fs import IMAGE HOST_PATH GUEST_PATH
//! cargo xtask fs import --model IMAGE FILE
//! cargo xtask fs ls IMAGE [GUEST_PATH]
//! cargo xtask fs cat IMAGE GUEST_PATH
//! cargo xtask fs check IMAGE [--repair]
//! cargo xtask fs replay BASE LOG
//! cargo xtask fs case TARGET [--base IMAGE] [--caught]
//! ```
//!
//! - `mkfs` makes a GPT disk with one `kai-data` partition at 2 MiB (P2 plan
//!   decision 1), written with `diskmap` under its `kai-data` type GUID;
//!   kaifs's three volumes in it; and their first folders: `/home/user`,
//!   `/home/.trash`, `/system/settings`, `/system/state` and `/system/pkg`
//!   (decision 5). It refuses to overwrite an image unless `--force`. The other
//!   commands also find `kai-data` on task 2's case disks, beside `kai-cache`.
//! - `import` copies a host file or folder to a guest path, in one commit,
//!   replacing files that are there. `--model` writes a GGUF file as the
//!   sealed blob `/system/models/<sha256>.gguf` and names it the active model
//!   in `kai-data`'s sector 0 (ADR 0004 §8).
//! - `ls` and `cat` read; `check` runs `check()` and every volume's namespace
//!   rules. `--repair` applies kaifs's whole-disk repair to what fails them:
//!   space, history and each volume's namespace. A disk that does not mount is
//!   mounted from its newest commit whose volume table reads, with its
//!   allocator rebuilt.
//! - `replay` checks every crash state a QEMU blklogwrites log allows, from
//!   `BASE`, the image as it was before the run (spike S04-3, task 9).
//! - `case` is task 9's check of a case's data disk: `TARGET/kai-data.img` and
//!   `TARGET/kai-data.log` as the run left them, from `--base`, the image before
//!   the run, or from zeros if the guest formatted it. The log must account for
//!   the disk, then every crash state is replayed; `--caught` wants a violation.
//!
//! Guest paths are the guest's: `/home/...`, `/system/...` and
//! `/system/models/...` are the `home`, `system` and `models` volumes. An image
//! without a GPT is taken as kaifs over the whole file.

mod blklog;
mod disk;
mod model;
mod ops;
#[cfg(test)]
mod tests;

use crate::blklog::Expect;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: cargo xtask fs mkfs IMAGE [--size MIB] [--label TEXT] [--force]
       cargo xtask fs import IMAGE HOST_PATH GUEST_PATH
       cargo xtask fs import --model IMAGE FILE
       cargo xtask fs ls IMAGE [GUEST_PATH]
       cargo xtask fs cat IMAGE GUEST_PATH
       cargo xtask fs check IMAGE [--repair]
       cargo xtask fs replay BASE LOG
       cargo xtask fs case TARGET [--base IMAGE] [--caught]";

/// The default data disk: 256 MiB, enough for a dev loop and small on a full C:.
const DEFAULT_MIB: u64 = 256;

pub fn main(args: &[String]) -> Result<(), String> {
    let usage = || USAGE.to_string();
    let (command, rest) = args.split_first().ok_or_else(usage)?;
    let mut words: Vec<&str> = Vec::new();
    let mut flags: Vec<(&str, Option<&str>)> = Vec::new();
    let mut iter = rest.iter().map(String::as_str);
    while let Some(word) = iter.next() {
        match word {
            "--size" | "--label" | "--base" => {
                flags.push((word, Some(iter.next().ok_or_else(usage)?)))
            }
            "--force" | "--repair" | "--caught" | "--model" => flags.push((word, None)),
            w if w.starts_with("--") => return Err(format!("{w}: not an option of fs\n{USAGE}")),
            w => words.push(w),
        }
    }
    let flag = |name: &str| flags.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let path = |i: usize| words.get(i).map(PathBuf::from).ok_or_else(usage);
    match (command.as_str(), words.len()) {
        ("mkfs", 1) => {
            let mib = match flag("--size").flatten() {
                Some(text) => text
                    .parse::<u64>()
                    .map_err(|_| format!("--size {text}: a number of MiB"))?,
                None => DEFAULT_MIB,
            };
            let label = flag("--label").flatten().unwrap_or("kai-data");
            let done = ops::mkfs(&path(0)?, mib << 20, label, flag("--force").is_some())?;
            println!("xtask fs: {done}");
            Ok(())
        }
        ("import", 2) if flag("--model").is_some() => {
            let done = model::import_model(&path(0)?, &path(1)?)?;
            println!("xtask fs: {done}");
            Ok(())
        }
        ("import", 3) => {
            let done = ops::import(&path(0)?, Path::new(words[1]), words[2])?;
            println!("xtask fs: {done}");
            Ok(())
        }
        ("ls", 1 | 2) => {
            let at = words.get(1).copied().unwrap_or("/");
            ops::ls(&path(0)?, at)?
                .iter()
                .for_each(|line| println!("{line}"));
            Ok(())
        }
        ("cat", 2) => {
            let mut out = std::io::stdout().lock();
            ops::cat(&path(0)?, words[1], &mut out).map(|_| ())
        }
        ("check", 1) => {
            let lines = ops::check_image(&path(0)?, flag("--repair").is_some())?;
            lines.iter().for_each(|line| println!("{line}"));
            Ok(())
        }
        ("replay", 2) => {
            let (base, log) = (path(0)?, path(1)?);
            let image = std::fs::read(&base).map_err(|e| format!("{}: {e}", base.display()))?;
            let region = disk::locate(&mut disk::Bytes(&image), image.len() as u64)?;
            let bytes = std::fs::read(&log).map_err(|e| format!("{}: {e}", log.display()))?;
            let log = blklog::parse(&bytes)?;
            let tally = blklog::replay(&blklog::base(&image, region), &log, region)?;
            println!(
                "xtask fs replay: {} crash states, {} violations",
                tally.states, tally.violations
            );
            for example in tally.examples.iter().take(5) {
                println!("  {example}");
            }
            match tally.violations {
                0 => Ok(()),
                n => Err(format!("{n} crash states do not check")),
            }
        }
        ("case", 1) => {
            let base = flag("--base").flatten().map(PathBuf::from);
            let expect = match flag("--caught") {
                Some(_) => Expect::Caught,
                None => Expect::Clean,
            };
            let figures = blklog::check_case(&path(0)?, base.as_deref(), expect)?;
            println!("xtask fs: {figures}");
            Ok(())
        }
        _ => Err(usage()),
    }
}
