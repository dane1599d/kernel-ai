//! S11-2's harness over kaifs (ADR 0011 §5; P2 task 13): random install, remove,
//! rollback, GC and probation sequences on a recording disk, then every crash
//! state kaifs's enumerator rebuilds. Each must mount, pass the store's
//! invariants and the switch's own (the grants are `current`'s, the newest switch
//! line names `current`), and equal the commit it holds, never older than the
//! last commit waited on.
//!
//! `KAIPKG_SEQUENCES` sets how many sequences run (default 40); the full gate is
//! 10,000: `KAIPKG_SEQUENCES=10000 cargo test --release -p kaipkg --test crash`.

mod common;

use common::*;
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format;
use kaifs::{Fs, Image, MemDisk, RecordingDisk};
use kaipkg::generation::{Generation, Time};
use kaipkg::resolve::Want;
use kaipkg::store::{self, Ctx, Error, Ledger, Policy, Refused, Store, Volume};
use std::collections::BTreeMap;

const BLOCKS: u64 = 8192;
const MAX_OPS: u64 = 12;
const DAY: u64 = 86_400;
/// The time the invariants are checked at: before every generation, so every
/// generation present counts as kept and its objects must all be there.
const CHECK: Time = Time(0);

fn sequences() -> u64 {
    std::env::var("KAIPKG_SEQUENCES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(40)
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Install(&'static str, Tamper),
    Remove(&'static str),
    /// Back to the `n`th generation present, modulo how many there are.
    Rollback(u64),
    Gc,
    MarkGood,
    FailProbation,
}

fn ops(rng: &mut Rng) -> Vec<Op> {
    let len = 1 + rng.below(MAX_OPS);
    (0..len)
        .map(|_| {
            let name = REPOSITORY[rng.below(REPOSITORY.len() as u64) as usize].name;
            match rng.below(100) {
                0..35 => Op::Install(name, Tamper::None),
                35..39 => Op::Install(name, Tamper::FlipByte),
                39..43 => Op::Install(name, Tamper::WrongHeader),
                43..58 => Op::Remove(name),
                58..72 => Op::Rollback(rng.next_u64()),
                72..84 => Op::Gc,
                84..93 => Op::MarkGood,
                _ => Op::FailProbation,
            }
        })
        .collect()
}

/// The deliberate fault: a ledger that commits the generation and `current`
/// before it writes the grants and the log line, so a switch is two
/// transactions. The harness must see it.
struct SplitLedger;

impl Ledger for SplitLedger {
    fn switched(&mut self, vol: &mut dyn Volume, n: u64, next: &Generation) -> Result<(), String> {
        vol.commit()?;
        vol.begin(1_000_000 + n)?;
        TestLedger.switched(vol, n, next)
    }

    fn collected(
        &mut self,
        vol: &mut dyn Volume,
        gens: &[u64],
        objects: usize,
    ) -> Result<(), String> {
        TestLedger.collected(vol, gens, objects)
    }
}

#[derive(Default)]
struct Report {
    sequences: u64,
    ops: u64,
    switches: u64,
    refused: BTreeMap<String, u64>,
    tampered: u64,
    tamper_refused: u64,
    tally: Tally,
}

impl Report {
    fn violation(&mut self, what: String) {
        self.tally.violations += 1;
        if self.tally.examples.len() < 8 {
            self.tally.examples.push(what);
        }
    }

    fn print(&self, what: &str) {
        eprintln!(
            "kaipkg crash, {what}: {} sequences, {} operations, {} switches; refused {:?}; \
             tamper cases {} of {} refused by name; {} crash states, {} violations",
            self.sequences,
            self.ops,
            self.switches,
            self.refused,
            self.tamper_refused,
            self.tampered,
            self.tally.states,
            self.tally.violations
        );
        for example in self.tally.examples.iter().take(6) {
            eprintln!("  e.g. {example}");
        }
    }
}

/// Mounts each crash state and compares it with the commit it holds.
struct StoreOracle<'a> {
    store: Store,
    expected: &'a BTreeMap<u64, Result<State, String>>,
}

impl Oracle for StoreOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        let fs = Fs::mount(&mut *image).map_err(|e| format!("mount: {e:?}"))?;
        let generation = fs.committed;
        if generation < min {
            return Err(format!(
                "mounted {generation}, older than the waited-on {min}"
            ));
        }
        let mut vol = KaifsVol::new(fs);
        let got = state(&mut vol, &self.store, CHECK)?;
        match self.expected.get(&generation) {
            Some(Ok(want)) if *want == got => Ok(()),
            Some(Ok(_)) => Err(format!("generation {generation} differs from its commit")),
            Some(Err(why)) => Err(format!("generation {generation} was bad when made: {why}")),
            None => Err(format!("generation {generation} was never committed")),
        }
    }
}

fn run_sequence(seed: u64, ledger: &mut dyn Ledger, report: &mut Report) -> Result<(), String> {
    let store = Store {
        policy: Policy {
            keep_last: 3,
            keep_secs: 4 * DAY,
        },
    };
    let mut uuid = *b"kaipkg crash 000";
    uuid[..8].copy_from_slice(&seed.to_le_bytes());
    let fs =
        Fs::mkfs(RecordingDisk::new(BLOCKS), uuid, "pkg crash").map_err(|e| format!("{e:?}"))?;
    let mut vol = KaifsVol::new(fs);
    vol.log_len = |fs| fs.io.dev.log.len();
    let mut now = Time::parse("2026-11-02T10:31:00Z").unwrap();
    let mut cx = Ctx {
        vol: &mut vol,
        ledger: &mut TestLedger,
        now,
        action: 1,
    };
    store
        .setup(&mut cx, Vec::new())
        .map_err(|e| format!("setup: {e:?}"))?;
    vol.fs.io.dev.checkpoint();
    let base: MemDisk = vol.fs.io.dev.disk.clone();
    let first = state(&mut vol, &store, CHECK);
    let mut expected = BTreeMap::from([(vol.fs.committed, first)]);
    let mut waited = vec![(0usize, vol.fs.committed)];
    vol.track = Some((store, CHECK));

    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for (action, op) in (2..).zip(ops(&mut rng)) {
        report.ops += 1;
        now.0 += DAY;
        let before = store::current(&mut vol)?;
        let real = |name: &str| kaipkg::sha256(&object(find(name)));
        let present_before = match op {
            Op::Install(name, _) => store::present(&mut vol, &real(name))?,
            _ => false,
        };
        let mut cx = Ctx {
            vol: &mut vol,
            ledger: &mut *ledger,
            now,
            action,
        };
        let result = match op {
            Op::Install(name, tamper) => {
                let (index, mut repo) = spoiled(name, tamper);
                repo.piece = 4 << 10;
                let want = [Want { name, range: None }];
                store.install(&mut cx, &mut repo, &index, OS, &want, false)
            }
            Op::Remove(name) => store.remove(&mut cx, &index(), OS, name),
            Op::Rollback(k) => {
                let gens = store::generations(cx.vol)?;
                store.rollback(&mut cx, gens[(k % gens.len() as u64) as usize])
            }
            Op::Gc => store.gc(&mut cx).map(|_| 0),
            Op::MarkGood => store.mark_good(&mut cx),
            Op::FailProbation => store.fail_probation(&mut cx),
        };
        if let Op::Install(_, tamper) = op
            && tamper != Tamper::None
        {
            let want = match tamper {
                Tamper::FlipByte => "BadHash",
                _ => "HeaderDiffers",
            };
            match &result {
                Ok(_) if !present_before => {
                    report.tampered += 1;
                    report.violation(format!("seed {seed}: tamper case {op:?} installed"));
                }
                Err(Error::Refused(why)) if format!("{why:?}").starts_with(want) => {
                    report.tampered += 1;
                    report.tamper_refused += 1;
                }
                Err(Error::Refused(Refused::BadHash(_) | Refused::HeaderDiffers(_))) => {
                    report.tampered += 1;
                    report.violation(format!("seed {seed}: {op:?} refused as {result:?}"));
                }
                _ => {}
            }
        }
        match result {
            Ok(_) => {
                if store::current(&mut vol)? != before {
                    report.switches += 1;
                }
            }
            Err(Error::Refused(why)) => {
                let kind = format!("{why:?}");
                let kind = kind.split('(').next().unwrap_or("").to_string();
                *report.refused.entry(kind).or_default() += 1;
            }
            Err(Error::Store(why)) => return Err(format!("{op:?}: {why}")),
        }
    }
    for (at, generation, s) in std::mem::take(&mut vol.commits) {
        if let Err(why) = &s {
            report.violation(format!("seed {seed}: live commit {generation}: {why}"));
        }
        waited.push((at, generation));
        expected.insert(generation, s);
    }
    let slots: Vec<u64> = format::all_slots(vol.fs.block_count()).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let recording = Recording {
        base: &base,
        log: &vol.fs.io.dev.log,
        waited: &waited,
        tear: &tear,
    };
    let options = Options {
        exhaustive: 4,
        random_subsets: 24,
        torn_subsets: 4,
    };
    let mut oracle = StoreOracle {
        store,
        expected: &expected,
    };
    let tally = enumerate(&recording, &options, &mut oracle, &mut rng);
    report.tally.add(&tally);
    Ok(())
}

fn run(ledger: &mut dyn Ledger, sequences: u64) -> Report {
    let mut report = Report::default();
    for seed in 0..sequences {
        report.sequences += 1;
        if let Err(why) = run_sequence(seed, ledger, &mut report) {
            report.violation(format!("seed {seed}: {why}"));
        }
    }
    report
}

/// The gate: ADR 0011's switch as one kaifs transaction holds in every crash
/// state, and every tamper case that reached a download is refused by name.
#[test]
fn one_transaction_per_switch_survives_every_crash() {
    let report = run(&mut TestLedger, sequences());
    report.print("one transaction a switch");
    assert_eq!(report.tally.violations, 0);
    assert!(report.switches > 0 && report.tally.states > 0);
    assert!(report.tampered > 0);
    assert_eq!(report.tamper_refused, report.tampered);
}

/// The harness must see a switch split across two transactions.
#[test]
fn a_switch_split_in_two_transactions_is_caught() {
    let report = run(&mut SplitLedger, sequences().div_ceil(4));
    report.print("the switch split in two (deliberate fault)");
    assert!(report.tally.violations > 0);
}
