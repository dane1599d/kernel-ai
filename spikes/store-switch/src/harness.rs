//! S11-2's crash harness: random install, remove, rollback, gc and probation
//! sequences, a crash injected at every write, flush and commit, and the store's
//! invariants checked in each state the crash left.
//!
//! - **kaifs**: the S04-1 engine's disk log is replayed to every flush for every
//!   sequence, and to every write for one sequence in `every_write`. Each state is
//!   mounted. It must equal the commit whose sequence number it holds, hold every
//!   commit whose two flushes completed, and pass [`store::check`].
//! - **Design C's primitives**: the files after every primitive, and with each
//!   write-and-flush torn in half, must pass [`store::check`] after a crash.

use crate::kaifs_disk::Kaifs;
use crate::prims::Prims;
use crate::store::{self, Layout, Op, Refused, Store, Summary, Tamper};
use crate::{Disk, repo};
use spike_kaifs::{CsumKind, Fs, Image, Op as DiskOp, RecordingDisk};
use std::collections::BTreeMap;

/// The kaifs disk: 32 MiB of 4 KiB blocks.
const BLOCKS: u64 = 8192;
/// Most operations in one sequence.
const MAX_OPS: u64 = 16;

#[derive(Debug, Default)]
pub struct Report {
    pub sequences: u64,
    pub ops: u64,
    /// Operations that switched `current` (installs, removes, rollbacks, failed
    /// probations).
    pub switches: u64,
    pub refused: BTreeMap<String, u64>,
    /// Tamper cases that reached a download, and those refused with the right
    /// named error.
    pub tampered: u64,
    pub tamper_refused: u64,
    /// Crash states checked, and how many of them came from every write.
    pub states: u64,
    pub write_states: u64,
    pub violations: u64,
    pub kinds: BTreeMap<String, u64>,
    pub examples: Vec<String>,
    /// kaifs: commits and blocks written by switches that fetched nothing.
    pub plain_switches: u64,
    pub plain_commits: u64,
    pub plain_blocks: u64,
    /// Design C: primitives done by switches that fetched nothing.
    pub plain_prims: u64,
}

impl Report {
    fn violation(&mut self, what: String) {
        self.violations += 1;
        // The reason, with its numbers taken out, so like violations count together.
        let reason = what.rsplit(": ").next().unwrap_or(&what);
        let mut kind = String::new();
        for c in reason.chars() {
            if !c.is_ascii_digit() {
                kind.push(c);
            } else if !kind.ends_with('#') {
                kind.push('#');
            }
        }
        *self.kinds.entry(kind).or_default() += 1;
        if self.examples.len() < 8 {
            self.examples.push(what);
        }
    }
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

/// A random sequence: installs most often, with a tamper case one time in ten.
pub fn random_ops(seed: u64) -> Vec<Op> {
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let len = 1 + xorshift(&mut rng) % MAX_OPS;
    let names: Vec<&'static str> = repo::REPOSITORY.iter().map(|p| p.name).collect();
    (0..len)
        .map(|_| {
            let name = names[(xorshift(&mut rng) % names.len() as u64) as usize];
            match xorshift(&mut rng) % 100 {
                0..35 => Op::Install(name, Tamper::None),
                35..38 => Op::Install(name, Tamper::FlipByte),
                38..41 => Op::Install(name, Tamper::WrongHeader),
                41..56 => Op::Remove(name),
                56..71 => Op::Rollback(xorshift(&mut rng) as usize),
                71..84 => Op::Gc,
                84..93 => Op::MarkGood,
                _ => Op::FailProbation,
            }
        })
        .collect()
}

/// Whether `op`'s tamper case will reach a download, and the refusal it must get.
fn tamper_case(disk: &mut dyn Disk, op: Op) -> Option<Refused> {
    let Op::Install(name, tamper) = op else {
        return None;
    };
    let package = repo::find(name)?;
    let sha = repo::index_sha(package);
    let present = disk
        .verified(&format!("pkg/store/{sha}"), &sha)
        .unwrap_or(false);
    let n = read_number(disk, "pkg/current");
    let current = store::generation(disk, n).ok()?;
    if present
        || current
            .installed
            .iter()
            .any(|p| p.name == name && p.explicit)
    {
        return None;
    }
    match tamper {
        Tamper::None => None,
        Tamper::FlipByte => Some(Refused::BadHash),
        Tamper::WrongHeader => Some(Refused::HeaderDiffers),
    }
}

fn read_number(disk: &mut dyn Disk, path: &str) -> u64 {
    disk.read(path)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok()?.trim().parse().ok())
        .unwrap_or(0)
}

/// Runs `ops` on the live disk, counting what happened.
fn run_ops(
    disk: &mut dyn Disk,
    store: &Store,
    ops: &[Op],
    report: &mut Report,
) -> Result<(), String> {
    for &op in ops {
        report.ops += 1;
        let expected = tamper_case(disk, op);
        let before = read_number(disk, "pkg/current");
        match store.run(disk, op)? {
            Ok(()) => {
                if read_number(disk, "pkg/current") != before {
                    report.switches += 1;
                }
                if expected.is_some() {
                    report.tampered += 1;
                    report.violation(format!("tamper case {op:?} installed"));
                }
            }
            Err(why) => {
                *report.refused.entry(format!("{why:?}")).or_default() += 1;
                if let Some(want) = expected {
                    report.tampered += 1;
                    if why == want {
                        report.tamper_refused += 1;
                    } else {
                        report.violation(format!(
                            "tamper case {op:?} refused as {why:?}, not {want:?}"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_plain_switch(op: Op) -> bool {
    matches!(op, Op::Remove(_) | Op::Rollback(_) | Op::FailProbation)
}

/// The harness over kaifs: `layout`'s switch as one transaction.
pub fn kaifs(store: Store, sequences: u64, every_write: u64) -> Report {
    let mut report = Report::default();
    for sequence in 0..sequences {
        report.sequences += 1;
        if let Err(why) = kaifs_sequence(store, sequence, every_write, &mut report) {
            report.violation(format!("sequence {sequence}: {why}"));
        }
    }
    report
}

fn kaifs_sequence(
    store: Store,
    sequence: u64,
    every_write: u64,
    report: &mut Report,
) -> Result<(), String> {
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&sequence.to_le_bytes());
    let fs = Fs::format(RecordingDisk::new(BLOCKS), CsumKind::Seahash, uuid)
        .map_err(|e| format!("{e:?}"))?;
    let mut disk = Kaifs::new(fs);
    disk.check = Some(store);
    store.setup(&mut disk);
    let setup_seq = disk.seq - 1;
    disk.fs.io.dev.checkpoint();
    let base = disk.fs.io.dev.disk.clone();
    let ops = random_ops(sequence);
    for &op in &ops {
        let (commits, blocks) = (disk.seq, disk.fs.io.dev.writes);
        run_ops(&mut disk, &store, &[op], report)?;
        if is_plain_switch(op) && disk.seq > commits {
            report.plain_switches += 1;
            report.plain_commits += disk.seq - commits;
            report.plain_blocks += disk.fs.io.dev.writes - blocks;
        }
    }
    let summaries = std::mem::take(&mut disk.summaries);
    for (seq, summary) in &summaries {
        if let Err(why) = summary {
            report.violation(format!("live commit {seq}: {why}"));
        }
    }
    let log = std::mem::take(&mut disk.fs.io.dev.log);
    let flushes = log.iter().filter(|op| matches!(op, DiskOp::Flush)).count() as u64;
    if flushes != 2 * (disk.seq - 1 - setup_seq) {
        report.violation(format!(
            "sequence {sequence}: {flushes} flushes for {} commits",
            disk.seq - 1 - setup_seq
        ));
    }
    let every = every_write > 0 && sequence.is_multiple_of(every_write);
    let mut flushed = 0u64;
    for k in 0..=log.len() {
        if k > 0 && matches!(log[k - 1], DiskOp::Flush) {
            flushed += 1;
        } else if k > 0 && !every {
            continue;
        }
        report.states += 1;
        if every {
            report.write_states += 1;
        }
        let durable = setup_seq + flushed / 2;
        if let Err(why) = crash_state(&base, &log[..k], durable, &summaries, store) {
            report.violation(format!(
                "sequence {sequence}, crash after disk op {k}: {why}"
            ));
        }
    }
    Ok(())
}

fn crash_state(
    base: &spike_kaifs::MemDisk,
    log: &[DiskOp],
    durable: u64,
    summaries: &BTreeMap<u64, Result<Summary, String>>,
    store: Store,
) -> Result<(), String> {
    let mut image = Image::new(base);
    for op in log {
        if let DiskOp::Write(addr, block) = op {
            image.apply(*addr, block);
        }
    }
    let fs = Fs::mount(image).map_err(|e| format!("does not mount: {e:?}"))?;
    let mut crashed = Kaifs::new(fs);
    let seq = crashed.committed_seq()?;
    if seq < durable {
        return Err(format!(
            "holds commit {seq}, older than {durable}, whose flushes completed"
        ));
    }
    let got = store::check(&mut crashed, &store)?;
    match summaries.get(&seq) {
        Some(Ok(want)) if *want == got => Ok(()),
        Some(Ok(_)) => Err(format!("differs from commit {seq} as it was made")),
        _ => Err(format!("holds commit {seq}, which was never made")),
    }
}

/// The harness over design C's three primitives.
pub fn prims(store: Store, sequences: u64) -> Report {
    let mut report = Report::default();
    for sequence in 0..sequences {
        report.sequences += 1;
        let mut disk = Prims::default();
        store.setup(&mut disk);
        disk.log.clear();
        let base = disk.clone();
        let ops = random_ops(sequence);
        for &op in &ops {
            let before = disk.log.len();
            if let Err(why) = run_ops(&mut disk, &store, &[op], &mut report) {
                report.violation(format!("sequence {sequence}: {why}"));
            }
            if is_plain_switch(op) && disk.log.len() > before {
                report.plain_switches += 1;
                report.plain_prims += (disk.log.len() - before) as u64;
            }
        }
        let log = std::mem::take(&mut disk.log);
        for done in 0..=log.len() {
            for torn in [false, true] {
                if torn && !matches!(log.get(done), Some(crate::prims::Prim::Write(..))) {
                    continue;
                }
                report.states += 1;
                let mut state = base.crashed(&log, done, torn);
                if let Err(why) = store::check(&mut state, &store) {
                    let how = if torn { ", torn" } else { "" };
                    report.violation(format!(
                        "sequence {sequence}, crash after primitive {done}{how}: {why}"
                    ));
                }
            }
        }
    }
    report
}

impl Report {
    pub fn print(&self, name: &str) {
        println!("S11-2 {name}:");
        println!(
            "  {} sequences, {} operations, {} switches; refused: {:?}",
            self.sequences, self.ops, self.switches, self.refused
        );
        println!(
            "  tamper cases: {} of {} refused with the right named error",
            self.tamper_refused, self.tampered
        );
        println!(
            "  {} crash states checked ({} of them at every write), {} violations",
            self.states, self.write_states, self.violations
        );
        if self.plain_switches > 0 && self.plain_commits > 0 {
            println!(
                "  a switch that fetches nothing: {:.2} commits, {:.1} blocks written",
                self.plain_commits as f64 / self.plain_switches as f64,
                self.plain_blocks as f64 / self.plain_switches as f64
            );
        }
        if self.plain_switches > 0 && self.plain_prims > 0 {
            println!(
                "  a switch that fetches nothing: {:.1} primitives",
                self.plain_prims as f64 / self.plain_switches as f64
            );
        }
        for (kind, n) in &self.kinds {
            println!("  violation kind {kind:?}: {n}");
        }
        for example in &self.examples {
            println!("  e.g. {example}");
        }
    }
}

/// The layouts the harness runs: ADR 0011's, and design C's for comparison.
pub fn store(layout: Layout) -> Store {
    Store {
        layout,
        keep_last: 4,
        split_switch: false,
    }
}
