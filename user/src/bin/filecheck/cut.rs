//! The power cut's workload (P2 task 9, ADR 0004 S04-3): file actions through
//! `fs`, each its own action, so a cut at any moment must leave every file as
//! some action committed it, never torn or mixed.
//!
//! Every file says what it should hold: a first line `kai-cut id=I v=V len=N`,
//! then bytes only `I` and `V` decide, `N` in all. [`verify`] needs no record of
//! what was written before the cut: a file is whole if it agrees with itself.
//!
//! - `step=actions` (`fs-actions`, behind blklogwrites): 200 actions of five
//!   kinds (create, overwrite, rename, trash, and every tenth a restore of the
//!   last trash), with each commit's latency for ADR 0002's typing-to-done table.
//! - `step=churn boot=N` (`fs-powercut`): checks every file, leaves
//!   `verified-N`, then makes actions until xtask cuts the power.

use super::{ask, dir_rights, expect, file_rights, mark, open, read_all};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{How, Kind, Reply, Request};
use kabi::handle::{Handle, Rights};
use user::{println, sys};

pub const ACTIONS: u64 = 200;
/// The folder under `/home/user` the files live in.
const DIR: &str = "cut";
const HEADER: &str = "kai-cut";

/// What the actions have left: live files by name with their id and version,
/// and the commit of the last trash, for the next restore.
#[derive(Default)]
struct State {
    live: Vec<(String, u64, u64)>,
    trashed: Option<u64>,
    commits: Vec<u64>,
}

pub fn actions(home: Handle) -> Result<(), String> {
    let dir = open(home, DIR, How::Mkdir, dir_rights())?;
    let mut state = State::default();
    for i in 0..ACTIONS {
        step(dir, i, &mut state)?;
    }
    let (p50, p99) = percentiles(&mut state.commits);
    println!(
        "filecheck: {ACTIONS} file actions committed; commit p50 {} us, p99 {} us ({} commits)",
        p50 / 1000,
        p99 / 1000,
        state.commits.len()
    );
    let files = verify(dir)?;
    mark(home, "actions.ok")?;
    println!("filecheck: {files} files whole after the actions");
    Ok(())
}

/// Checks every file, then makes actions until the power is cut.
pub fn churn(home: Handle, boot: u64) -> Result<(), String> {
    let dir = match open(home, DIR, How::Existing, dir_rights()) {
        Ok(dir) => dir,
        Err(_) => open(home, DIR, How::Mkdir, dir_rights())?,
    };
    let files = verify(dir)?;
    println!("filecheck: boot {boot}: {files} files whole after the cut");
    mark(home, &format!("verified-{boot}"))?;
    let mut state = State {
        live: listing(dir)?.into_iter().map(|name| (name, 0, 0)).collect(),
        ..State::default()
    };
    let mut i = boot * 1_000_000;
    loop {
        step(dir, i, &mut state)?;
        i += 1;
    }
}

/// Action `i`; a restore every tenth, else one of four changes.
fn step(dir: Handle, i: u64, state: &mut State) -> Result<(), String> {
    if i % 10 == 9
        && let Some(txn) = state.trashed.take()
    {
        let request = Request::Restore { txn, action: i + 1 };
        return match ask(dir, request)? {
            (Reply::Restored { .. }, _) => {
                // What came back is listed again by name.
                state.live = listing(dir)?.into_iter().map(|name| (name, 0, 0)).collect();
                Ok(())
            }
            other => Err(format!("restore of {txn}: {other:?}")),
        };
    }
    expect(dir, Request::Begin { action: i + 1 }, Reply::Done)?;
    let pick = |state: &State| (i as usize * 7) % state.live.len().max(1);
    match (i % 5, state.live.is_empty()) {
        (0 | 1, _) | (_, true) => {
            let name = format!("f{i}");
            write_file(dir, &name, How::CreateFile, i, 1)?;
            state.live.push((name, i, 1));
        }
        (2, false) => {
            let at = pick(state);
            let (name, id, version) = state.live[at].clone();
            // A file found by listing gets a new id: its old one is unknown.
            let (id, version) = if version == 0 {
                (i, 1)
            } else {
                (id, version + 1)
            };
            write_file(dir, &name, How::Existing, id, version)?;
            state.live[at] = (name, id, version);
        }
        (3, false) => {
            let at = pick(state);
            let to = format!("r{i}");
            let request = Request::Rename {
                from: state.live[at].0.clone(),
                to: to.clone(),
            };
            expect(dir, request, Reply::Done)?;
            state.live[at].0 = to;
        }
        _ => {
            let at = pick(state);
            let (name, ..) = state.live.remove(at);
            expect(dir, Request::Trash { name }, Reply::Done)?;
        }
    }
    let trashing = i % 5 == 4;
    let started = sys::time_ns();
    match ask(dir, Request::Commit)? {
        (Reply::Committed { generation }, _) => {
            state.commits.push(sys::time_ns().saturating_sub(started));
            if trashing {
                state.trashed = Some(generation);
            }
            Ok(())
        }
        other => Err(format!("commit of action {}: {other:?}", i + 1)),
    }
}

/// Writes the whole of version `version` of file `id` as `name`.
fn write_file(dir: Handle, name: &str, how: How, id: u64, version: u64) -> Result<(), String> {
    let file = open(dir, name, how, file_rights())?;
    let bytes = contents(id, version);
    let done = (|| {
        expect(file, Request::Truncate { size: 0 }, Reply::Done)?;
        for (at, piece) in bytes.chunks(fsproto::MAX_DATA).enumerate() {
            let request = Request::Write {
                off: (at * fsproto::MAX_DATA) as u64,
                data: piece.to_vec(),
            };
            expect(file, request, Reply::Done)?;
        }
        Ok(())
    })();
    let _ = sys::handle_close(file);
    done
}

/// What version `version` of file `id` holds: 1 to 20 KiB.
fn contents(id: u64, version: u64) -> Vec<u8> {
    let len = 1024 + ((id * 7919 + version * 104_729) % (19 * 1024)) as usize;
    let mut bytes = format!("{HEADER} id={id} v={version} len={len}\n").into_bytes();
    let seed = id.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ version;
    let mut x = seed | 1;
    while bytes.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        bytes.push(x as u8);
    }
    bytes
}

/// The names in `dir`.
fn listing(dir: Handle) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut after = None;
    loop {
        let request = Request::List {
            after,
            max: fsproto::MAX_LIST,
        };
        match ask(dir, request)? {
            (Reply::Listed { names: got, more }, _) => {
                names.extend(
                    got.into_iter()
                        .filter(|l| l.kind == Kind::File)
                        .map(|l| l.name),
                );
                match more {
                    Some(cursor) => after = Some(cursor),
                    None => return Ok(names),
                }
            }
            other => return Err(format!("listing: {other:?}")),
        }
    }
}

/// Every file in `dir` agrees with its own first line; how many there are.
pub fn verify(dir: Handle) -> Result<usize, String> {
    let names = listing(dir)?;
    for name in &names {
        let file = open(dir, name, How::Existing, Rights::READ)?;
        let size = match ask(file, Request::Stat)? {
            (Reply::Stat(stat), _) => stat.size as usize,
            other => return Err(format!("{name}: {other:?}")),
        };
        let bytes = read_all(file, size);
        let _ = sys::handle_close(file);
        let bytes = bytes?;
        let line = bytes.split(|&b| b == b'\n').next().unwrap_or(&[]);
        let line = core::str::from_utf8(line).unwrap_or("");
        let field = |key: &str| {
            line.split_whitespace()
                .find_map(|w| w.strip_prefix(key))
                .and_then(|v| v.parse::<u64>().ok())
        };
        let whole = match (
            line.starts_with(HEADER),
            field("id="),
            field("v="),
            field("len="),
        ) {
            (true, Some(id), Some(version), Some(len)) => {
                len as usize == size && bytes == contents(id, version)
            }
            _ => false,
        };
        if !whole {
            return Err(format!("{name} is torn: {size} bytes, first line {line:?}"));
        }
    }
    Ok(names.len())
}

fn percentiles(samples: &mut [u64]) -> (u64, u64) {
    samples.sort_unstable();
    let at = |p: usize| samples.get((samples.len() * p / 100).min(samples.len().saturating_sub(1)));
    (at(50).copied().unwrap_or(0), at(99).copied().unwrap_or(0))
}
