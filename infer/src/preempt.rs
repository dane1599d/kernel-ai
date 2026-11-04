//! S13-1 `kv-preempt`, the guest half (ADR 0013 section 2): what it costs, with the
//! real model, to snapshot a KV slot into RAM, restore it, and spill it to a file
//! and back, at a few sizes, and whether what comes back answers as the slot did.
//!
//! [`measure`] is the whole measurement, on a caller's engine, clock and spill
//! target, so the `llm` service runs it on the model disk's file in the guest (the
//! spill goes to the `kai-cache` partition) and the host tests run it on a fixture
//! model. For each size it fills the slot with a prompt of about that many tokens,
//! takes a snapshot three times, spills the slot, and asks a follow-up of the live
//! slot for a reference answer. It then restores the snapshot, and again the
//! spill file, each three times, and asks the follow-up of each restored slot. The
//! restored slot must have the prompt's positions in it, and its answer must be
//! the live slot's, byte for byte. Nothing passes or fails on the times: they are
//! read from the report against the bars (a RAM snapshot and restore of 1,024
//! positions in at most 50 ms, a spill round trip in at most 1.5 s).

use crate::chat::{Engine, Reply, Role, Turn};
use crate::segments::{Key, Snapshot, spill_pack_id};
use crate::{Next, Sampler};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Tokens the follow-up asks the answer to make.
const ANSWER: usize = 8;
/// Positions kept free after the prompt, for the follow-up and its answer.
const ROOM: usize = 48;
/// How often each step is timed.
const RUNS: usize = 3;
/// Tokens at the end of a prompt that a follow-up does not share with it: the
/// template's start of the assistant's turn.
const TEMPLATE: usize = 8;

/// What a spill file's bytes are written to, in order: false once one could not be
/// kept.
pub type Write<'a> = Box<dyn FnMut(&[u8]) -> bool + 'a>;
/// What a spill file is read from: it fills each buffer it is given, in order, and
/// is false past the file's end.
pub type Read<'a> = Box<dyn FnMut(&mut [u8]) -> bool + 'a>;

/// Where a spill file goes: the `kai-cache` partition's region in the guest, a
/// buffer on the host.
pub trait Spill {
    /// Starts a spill file: what it is given is written, in order. A write that
    /// fails returns false.
    fn writer(&mut self) -> Write<'_>;
    /// Ends the file started: whether every byte is durable.
    fn finish(&mut self) -> bool;
    /// Reads the file written from its start.
    fn reader(&mut self) -> Read<'_>;
}

/// What one size cost, in nanoseconds by the caller's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Positions the prompt filled.
    pub positions: usize,
    /// The prompt's prefill, and the one token after it.
    pub prefill_ns: u64,
    /// The first snapshot, which touches the reserved memory first, and the better of
    /// the next two.
    pub snapshot_first_ns: u64,
    pub snapshot_ns: u64,
    /// Median of [`RUNS`] restores from the snapshot.
    pub restore_ns: u64,
    /// The spill file written whole and durable, and read back (median of [`RUNS`]).
    pub spill_save_ns: u64,
    pub spill_restore_ns: u64,
    /// The restored slot held the prompt and answered the live slot's answer.
    pub snapshot_equal: bool,
    pub spill_equal: bool,
}

/// `ns` as milliseconds with a tenth.
fn ms(ns: u64) -> String {
    format!("{}.{}", ns / 1_000_000, (ns / 100_000) % 10)
}

impl Row {
    /// One line of the report.
    pub fn line(&self) -> String {
        format!(
            "{} positions: prefill {} ms; snapshot {} ms first, {} ms pooled; restore {} ms; \
             both {} ms; spill save {} ms, restore {} ms, both {} ms; \
             equal after restore: snapshot {}, spill {}",
            self.positions,
            ms(self.prefill_ns),
            ms(self.snapshot_first_ns),
            ms(self.snapshot_ns),
            ms(self.restore_ns),
            ms(self.snapshot_ns + self.restore_ns),
            ms(self.spill_save_ns),
            ms(self.spill_restore_ns),
            ms(self.spill_save_ns + self.spill_restore_ns),
            self.snapshot_equal,
            self.spill_equal,
        )
    }

    /// Whether what came back was what went out.
    pub fn equal(&self) -> bool {
        self.snapshot_equal && self.spill_equal
    }
}

/// The median of `times`, which has an odd length.
fn median(mut times: Vec<u64>) -> u64 {
    times.sort_unstable();
    times[times.len() / 2]
}

/// A user message that makes about `positions` tokens of conversation, and the
/// follow-up that continues it.
fn conversation(engine: &Engine<'_>, positions: usize) -> String {
    const SENTENCE: &str = "The quick brown fox jumps over the lazy dog near the river bank. ";
    let per = engine.text_tokens(SENTENCE).max(1);
    let tokens = |text: &str| engine.conversation_tokens(&[Turn::new(Role::User, text)]);
    let mut count = positions / per;
    let mut text = SENTENCE.repeat(count);
    // The template's own tokens first, then the repeat that makes up the rest.
    for _ in 0..8 {
        let have = tokens(&text);
        if have.abs_diff(positions) < per {
            break;
        }
        count = if have > positions {
            count.saturating_sub((have - positions) / per)
        } else {
            count + (positions - have) / per
        };
        text = SENTENCE.repeat(count);
    }
    text
}

/// Runs the measurement at each of `sizes` (positions the prompt fills, each at
/// most the slot's context less [`ROOM`]) in the slot in use, which it empties and
/// leaves holding something else. A size that cannot be measured says why instead of
/// a row. `done` is told each size's outcome as soon as it is known, so a run that
/// dies later still has the rows before it.
///
/// The snapshot's memory is taken once, for the largest size, and every size reuses
/// it: a buffer made and dropped per size fragments a heap that never gives pages
/// back, and a guest's quota is for one snapshot, not for each size's.
pub fn measure(
    engine: &mut Engine<'_>,
    key: &Key,
    sizes: &[usize],
    spill: &mut dyn Spill,
    clock: &dyn Fn() -> u64,
    pace: &dyn Fn(),
    done: &mut dyn FnMut(usize, &Result<Row, String>),
) -> Vec<Result<Row, String>> {
    let mut snapshot = Snapshot::new();
    let most = sizes
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_add(ROOM);
    let reserved = engine
        .reserve_snapshot(&mut snapshot, most)
        .map_err(|e| format!("snapshot memory: {e}"));
    sizes
        .iter()
        .map(|&size| {
            let row = match &reserved {
                Ok(()) => one(engine, key, size, &mut snapshot, spill, clock, pace),
                Err(why) => Err(why.clone()),
            };
            done(size, &row);
            row
        })
        .collect()
}

/// One reply, greedy, of at most `max_new` tokens.
fn ask(
    engine: &mut Engine<'_>,
    turns: &[Turn<'_>],
    max_new: usize,
    pace: &dyn Fn(),
) -> Result<Reply, String> {
    engine
        .reply(turns, max_new, &mut Sampler::greedy(), None, |_| {
            pace();
            Next::Continue
        })
        .map_err(|e| format!("{e}"))
}

fn one(
    engine: &mut Engine<'_>,
    base: &Key,
    size: usize,
    snapshot: &mut Snapshot,
    spill: &mut dyn Spill,
    clock: &dyn Fn() -> u64,
    pace: &dyn Fn(),
) -> Result<Row, String> {
    let context = engine.about().context;
    if size == 0 || size + ROOM > context {
        return Err(format!("{size} positions do not fit a slot of {context}"));
    }
    // A key of its own for this size: a file of another size is refused, not read.
    let key = Key {
        pack_id: spill_pack_id(0x13, size as u64),
        ..*base
    };
    let text = conversation(engine, size);
    let turns = [Turn::new(Role::User, &text)];
    let follow = [
        Turn::new(Role::User, &text),
        Turn::new(Role::Assistant, "Noted."),
        Turn::new(Role::User, "And what did it jump over?"),
    ];
    // The prompt, and one token: the slot holds the prompt.
    let start = clock();
    let filled = ask(engine, &turns, 1, pace)?;
    let prefill_ns = clock() - start;
    let positions = filled.prompt_tokens;
    // RAM: the first snapshot touches the reserved memory first, the others reuse it.
    let mut times = Vec::new();
    for _ in 0..RUNS {
        let start = clock();
        engine
            .snapshot_slot(&key, snapshot)
            .map_err(|e| format!("snapshot: {e}"))?;
        times.push(clock() - start);
    }
    // The first pays for the memory's first touch; the pooled time is the better of the others.
    let snapshot_first_ns = times[0];
    let snapshot_ns = times[1..].iter().copied().min().unwrap_or(0);
    // The spill file, written whole and made durable.
    let mut spill_times = Vec::new();
    for _ in 0..RUNS {
        let start = clock();
        let mut write = spill.writer();
        let wrote = engine.save_slot(&key, &mut |bytes| write(bytes));
        drop(write);
        let ok = wrote.is_ok() && spill.finish();
        spill_times.push(clock() - start);
        if !ok {
            return Err(String::from("the spill file was not written"));
        }
    }
    let spill_save_ns = median(spill_times);
    // The live slot's answer to the follow-up: what a restored slot must give.
    let live = ask(engine, &follow, ANSWER, pace)?;
    // Restored from RAM.
    let mut restore_times = Vec::new();
    let mut snapshot_equal = true;
    for _ in 0..RUNS {
        let start = clock();
        let reused = engine
            .restore_snapshot(&key, &follow, snapshot)
            .map_err(|why| format!("snapshot restore refused: {why:?}"))?;
        restore_times.push(clock() - start);
        let again = ask(engine, &follow, ANSWER, pace)?;
        snapshot_equal &= reused + TEMPLATE >= positions
            && again.cached + TEMPLATE >= positions
            && (again.bytes.as_slice(), again.tokens) == (live.bytes.as_slice(), live.tokens);
    }
    let restore_ns = median(restore_times);
    // Restored from the spill file.
    let mut spill_restore_times = Vec::new();
    let mut spill_equal = true;
    for _ in 0..RUNS {
        let start = clock();
        let mut read = spill.reader();
        let reused = engine.restore_slot(&key, &follow, &mut |buf| read(buf));
        drop(read);
        let reused = reused.map_err(|why| format!("spill restore refused: {why:?}"))?;
        spill_restore_times.push(clock() - start);
        let again = ask(engine, &follow, ANSWER, pace)?;
        spill_equal &= reused + TEMPLATE >= positions
            && again.cached + TEMPLATE >= positions
            && (again.bytes.as_slice(), again.tokens) == (live.bytes.as_slice(), live.tokens);
    }
    let spill_restore_ns = median(spill_restore_times);
    Ok(Row {
        positions,
        prefill_ns,
        snapshot_first_ns,
        snapshot_ns,
        restore_ns,
        spill_save_ns,
        spill_restore_ns,
        snapshot_equal,
        spill_equal,
    })
}
