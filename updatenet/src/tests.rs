//! The update's rules, over memory: the plan, the slot as a download writes it, and
//! the stage event around it with a scripted server.

use crate::plan::{self, Job, Machine, Refused};
use crate::slot::{Checkpoint, Mark, READ_BACK, SlotIo, SlotPartial};
use crate::stage::{Bad, Boot, Stop, stage};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bootslot::{Nv, Reason, Record, Slot, SlotState};
use kainet::chunks::{CHUNK, Chunks};
use kainet::download::Answer;
use kainet::http::{HttpError, Range};
use kaipkg::online::Hosts;
use kaituf::meta::TargetFile;
use serde_json::json;
use sha2::{Digest, Sha256};

const TAG: &str = "os-7";
const SEQ: u32 = 7;

/// A deterministic image of `chunks` whole chunks and a short last one.
fn image(chunks: usize, salt: u8) -> Vec<u8> {
    let len = chunks * CHUNK as usize + 777;
    let mut x = 0x2545_f491_4f6c_dd1du64 ^ u64::from(salt);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn image_target(bytes: &[u8], seq: u64, tag: &str) -> TargetFile {
    TargetFile {
        path: String::from(plan::TARGET),
        length: bytes.len() as u64,
        hashes: vec![(String::from("sha256"), digest(bytes))],
        custom: json!({ "kai": { "release_seq": seq, "tag": tag } }),
    }
}

fn list_target(list: &str) -> TargetFile {
    TargetFile {
        path: String::from(plan::CHUNKS_TARGET),
        length: list.len() as u64,
        hashes: vec![(String::from("sha256"), digest(list.as_bytes()))],
        custom: json!(null),
    }
}

fn hosts() -> Hosts {
    Hosts {
        metadata: String::from("https://updates.example/metadata/"),
        releases: String::from("https://releases.example/kai/releases/download/"),
        assets: String::from("assets.example"),
    }
}

/// A record whose running slot A holds release `seq`, committed.
fn record(seq: u32) -> Record {
    let mut record = Record::default();
    record.slots[Slot::A as usize] = SlotState {
        priority: 2,
        successful: true,
        release_seq: seq,
        ..SlotState::default()
    };
    record
}

fn machine(record: &Record) -> Machine<'_> {
    Machine {
        record,
        running: Slot::A,
        into: None,
        room: 64 << 20,
    }
}

/// The signed pair for `bytes` as release `seq`, and the job it plans.
struct Release7 {
    bytes: Vec<u8>,
    list: Chunks,
    text: String,
    job: Job,
}

fn release(chunks: usize, salt: u8) -> Release7 {
    let bytes = image(chunks, salt);
    let list = Chunks::of(&bytes, CHUNK);
    let text = list.render();
    let record = record(SEQ - 1);
    let job = plan::plan(
        &image_target(&bytes, u64::from(SEQ), TAG),
        &list_target(&text),
        &machine(&record),
        &hosts(),
    )
    .unwrap();
    Release7 {
        bytes,
        list,
        text,
        job,
    }
}

/// The metadata refreshed again, as it signs `r`: nothing has changed since.
fn signed(r: &Release7) -> impl FnMut() -> Result<(TargetFile, TargetFile), Refused> {
    let pair = (
        image_target(&r.bytes, u64::from(SEQ), TAG),
        list_target(&r.text),
    );
    move || Ok(pair.clone())
}

mod blocks;
mod recheck;
mod stage;

// ---- the plan

#[test]
fn a_release_is_planned_into_the_other_slot_with_its_files_under_the_tag() {
    let r = release(2, 0);
    assert_eq!(r.job.slot, Slot::B);
    assert_eq!(r.job.release.seq, SEQ);
    assert_eq!(r.job.release.tag, TAG);
    let sha = kaituf::hex(&digest(&r.bytes));
    assert_eq!(
        r.job.image_url,
        format!("https://releases.example/kai/releases/download/{TAG}/{sha}.slot.img")
    );
    let list_sha = kaituf::hex(&digest(r.text.as_bytes()));
    assert_eq!(
        r.job.list_url,
        format!("https://releases.example/kai/releases/download/{TAG}/{list_sha}.slot.img.chunks")
    );
    assert!(r.job.reaches("releases.example", 443));
    assert!(r.job.reaches("assets.example", 443));
    assert!(!r.job.reaches("evil.example", 443));
    assert!(!r.job.reaches("assets.example", 80));
}

#[test]
fn a_boot_in_b_writes_a_and_a_boot_in_r_writes_the_slot_it_names() {
    let mut record = record(SEQ - 1);
    record.slots[Slot::B as usize].successful = true;
    let mut m = machine(&record);
    m.running = Slot::B;
    assert_eq!(m.slot(), Ok(Slot::A));
    m.running = Slot::R;
    assert!(matches!(m.slot(), Err(Refused::Slot(_))));
    m.into = Some(Slot::R);
    assert!(matches!(m.slot(), Err(Refused::Slot(_))));
    m.into = Some(Slot::B);
    assert_eq!(m.slot(), Ok(Slot::B));
}

/// A trial never stages over the committed slot it would fall back to; with
/// neither committed (a fresh install), it may.
#[test]
fn a_trial_slot_never_stages_over_the_committed_one() {
    let record = record(SEQ - 1);
    let mut m = machine(&record);
    m.running = Slot::B;
    assert!(matches!(m.slot(), Err(Refused::Slot(_))));
    let fresh = Record::default();
    let mut m = machine(&fresh);
    assert_eq!(m.slot(), Ok(Slot::B));
    m.running = Slot::B;
    assert_eq!(m.slot(), Ok(Slot::A));
}

#[test]
fn a_release_that_is_not_newer_than_every_committed_one_is_refused() {
    let bytes = image(1, 1);
    let list = Chunks::of(&bytes, CHUNK).render();
    let go = |record: &Record, running: Slot, seq: u64| {
        let mut m = machine(record);
        m.running = running;
        m.into = Some(Slot::B);
        plan::plan(
            &image_target(&bytes, seq, TAG),
            &list_target(&list),
            &m,
            &hosts(),
        )
    };
    // The running slot's release, even on trial, is the floor.
    assert_eq!(
        go(&record(7), Slot::A, 7),
        Err(Refused::Rollback { seq: 7, newest: 7 })
    );
    assert!(go(&record(6), Slot::A, 7).is_ok());
    // The floor a commit raised, whatever slot runs.
    let mut floor = record(1);
    floor.min_release_seq = 9;
    assert_eq!(
        go(&floor, Slot::A, 9),
        Err(Refused::Rollback { seq: 9, newest: 9 })
    );
    assert!(go(&floor, Slot::A, 10).is_ok());
    // In R, a committed slot's release is the floor too.
    assert_eq!(
        go(&record(5), Slot::R, 5),
        Err(Refused::Rollback { seq: 5, newest: 5 })
    );
    assert!(go(&record(5), Slot::R, 6).is_ok());
}

#[test]
fn a_target_that_names_no_release_or_a_bad_tag_or_no_hash_is_refused() {
    let bytes = image(1, 2);
    let list = list_target(&Chunks::of(&bytes, CHUNK).render());
    let record = record(1);
    let m = machine(&record);
    let go = |target: TargetFile| plan::plan(&target, &list, &m, &hosts());
    let good = image_target(&bytes, 7, TAG);
    for (what, custom) in [
        ("no custom", json!(null)),
        ("no release_seq", json!({ "kai": { "tag": TAG } })),
        (
            "a zero release_seq",
            json!({ "kai": { "release_seq": 0, "tag": TAG } }),
        ),
        (
            "a huge release_seq",
            json!({ "kai": { "release_seq": 1u64 << 40, "tag": TAG } }),
        ),
        ("no tag", json!({ "kai": { "release_seq": 7 } })),
        (
            "a dot tag",
            json!({ "kai": { "release_seq": 7, "tag": ".." } }),
        ),
        (
            "a slash tag",
            json!({ "kai": { "release_seq": 7, "tag": "a/b" } }),
        ),
        (
            "an empty tag",
            json!({ "kai": { "release_seq": 7, "tag": "" } }),
        ),
        (
            "a long tag",
            json!({ "kai": { "release_seq": 7, "tag": "x".repeat(65) } }),
        ),
    ] {
        let target = TargetFile {
            custom,
            ..good.clone()
        };
        assert!(matches!(go(target), Err(Refused::Field(_))), "{what}");
    }
    let no_hash = TargetFile {
        hashes: Vec::new(),
        ..good.clone()
    };
    assert_eq!(go(no_hash), Err(Refused::Field("a target has no sha256")));
    let short = TargetFile {
        hashes: vec![(String::from("sha256"), vec![0; 5])],
        ..good
    };
    assert!(matches!(go(short), Err(Refused::Field(_))));
}

#[test]
fn an_image_longer_than_the_slot_or_already_staged_is_refused() {
    let bytes = image(2, 3);
    let list = list_target(&Chunks::of(&bytes, CHUNK).render());
    let target = image_target(&bytes, 7, TAG);
    let record = record(1);
    let mut m = machine(&record);
    m.room = bytes.len() as u64 - 1;
    assert_eq!(
        plan::plan(&target, &list, &m, &hosts()),
        Err(Refused::Room {
            need: bytes.len() as u64,
            room: bytes.len() as u64 - 1
        })
    );
    // The last write is padded to a whole 4 KiB block, which must fit too.
    m.room = bytes.len() as u64;
    let refused = plan::plan(&target, &list, &m, &hosts()).unwrap_err();
    assert!(matches!(refused, Refused::Room { .. }));
    // Refused by its pad alone: the words say so.
    assert!(
        refused.why().contains("in whole 4 KiB blocks"),
        "{}",
        refused.why()
    );
    m.room = crate::blocks::padded(bytes.len() as u64).unwrap();
    assert!(plan::plan(&target, &list, &m, &hosts()).is_ok());
    // The idle slot holds release 7 as a trial: staged already.
    let mut staged = record;
    staged.slots[Slot::B as usize] = SlotState {
        priority: 3,
        tries: 3,
        release_seq: 7,
        ..SlotState::default()
    };
    assert_eq!(
        plan::plan(&target, &list, &machine(&staged), &hosts()),
        Err(Refused::Staged(7))
    );
    // Half written (Updating) is not staged.
    staged.slots[Slot::B as usize].reason = Reason::Updating;
    staged.slots[Slot::B as usize].priority = 0;
    assert!(plan::plan(&target, &list, &machine(&staged), &hosts()).is_ok());
}

#[test]
fn the_chunk_list_must_be_the_signed_one_for_this_image_in_two_mib_chunks() {
    let r = release(2, 4);
    assert_eq!(r.job.chunks(r.text.as_bytes()).unwrap(), r.list);
    // Not the signed bytes.
    let mut other = r.text.clone().into_bytes();
    let last = other.len() - 2;
    other[last] ^= 1;
    assert_eq!(
        r.job.chunks(&other),
        Err(Refused::List("the chunk list is not the signed one"))
    );
    // A signed list for other bytes, or for the right bytes in other chunks.
    let wrong_blob = Chunks::of(&image(2, 5), CHUNK).render();
    let job = Job {
        list: list_target(&wrong_blob),
        ..r.job.clone()
    };
    assert_eq!(
        job.chunks(wrong_blob.as_bytes()),
        Err(Refused::List("the chunk list is for another image"))
    );
    let wide = Chunks::of(&r.bytes, 4 * CHUNK).render();
    let job = Job {
        list: list_target(&wide),
        ..r.job.clone()
    };
    assert_eq!(
        job.chunks(wide.as_bytes()),
        Err(Refused::List("the chunk list's chunks are not 2 MiB"))
    );
    // A list that is not a list at all, though signed.
    let junk = "kai-chunks 2\n";
    let job = Job {
        list: list_target(junk),
        ..r.job.clone()
    };
    assert!(matches!(job.chunks(junk.as_bytes()), Err(Refused::List(_))));
}

// ---- the slot, the checkpoint, the boot variable and the server

struct Mem {
    bytes: Vec<u8>,
    writes: Vec<(u64, usize)>,
    /// A write at this offset is dropped, as a bad sector drops it.
    drop_at: Option<u64>,
}

impl Mem {
    fn new(len: usize) -> Self {
        Self {
            bytes: vec![0; len],
            writes: Vec::new(),
            drop_at: None,
        }
    }
}

impl SlotIo for Mem {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
        self.writes.push((offset, bytes.len()));
        if self.drop_at == Some(offset) {
            return Ok(());
        }
        let end = offset as usize + bytes.len();
        if end > self.bytes.len() {
            return Err("past the end of the slot");
        }
        self.bytes[offset as usize..end].copy_from_slice(bytes);
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let end = offset as usize + buf.len();
        let bytes = self.bytes.get(offset as usize..end).ok_or("past the end")?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        Ok(())
    }
}

#[derive(Default)]
struct Memo {
    mark: Option<Mark>,
    saves: usize,
}

impl Checkpoint for Memo {
    fn load(&mut self) -> Result<Option<Mark>, &'static str> {
        Ok(self.mark)
    }

    fn save(&mut self, mark: Option<Mark>) -> Result<(), &'static str> {
        self.saves += 1;
        self.mark = mark;
        Ok(())
    }
}

/// The boot variable: every value written to it, in order.
#[derive(Default)]
struct Var {
    sets: Vec<Nv>,
    /// Fails the `n`th `set` (from 1).
    fail_nth: Option<usize>,
    /// How many `sets` there were each time `before_trial` was called.
    before_trial: Vec<usize>,
    /// Makes `before_trial` fail.
    fail_before_trial: bool,
}

impl Boot for Var {
    fn set(&mut self, nv: &Nv) -> Result<(), &'static str> {
        if self.fail_nth == Some(self.sets.len() + 1) {
            return Err("the boot variable cannot be written");
        }
        self.sets.push(*nv);
        Ok(())
    }

    fn before_trial(&mut self, _: &Job) -> Result<(), &'static str> {
        self.before_trial.push(self.sets.len());
        match self.fail_before_trial {
            true => Err("the snapshot cannot be taken"),
            false => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
enum Fault {
    /// The first range that holds this byte is cut just before it.
    CutOnce(usize),
    /// This byte of the file is flipped, whenever served.
    Flip(usize),
    /// The next answer is this status, with no body.
    Status(u16, Option<u32>),
    /// The next range's body is this many bytes short.
    Short(usize),
}

/// A server of the image by range, with its faults.
struct Server {
    bytes: Vec<u8>,
    faults: Vec<Fault>,
    served: Vec<(u64, u64)>,
}

impl Server {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            faults: Vec::new(),
            served: Vec::new(),
        }
    }

    fn with(mut self, fault: Fault) -> Self {
        self.faults.push(fault);
        self
    }

    fn fetch(
        &mut self,
        range: Range,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Answer, HttpError> {
        let first = range.first as usize;
        let last = range.last.expect("a bounded range") as usize;
        self.served.push((range.first, range.last.unwrap()));
        if let Some(at) = self
            .faults
            .iter()
            .position(|f| matches!(f, Fault::Status(..)))
        {
            let Fault::Status(status, retry_after) = self.faults.remove(at) else {
                unreachable!()
            };
            return Ok(Answer {
                status,
                retry_after,
            });
        }
        let mut body = self.bytes[first..=last].to_vec();
        for fault in &self.faults {
            if let Fault::Flip(at) = fault
                && (first..=last).contains(at)
            {
                body[at - first] ^= 1;
            }
        }
        if let Some(at) = self
            .faults
            .iter()
            .position(|f| matches!(f, Fault::Short(_)))
        {
            let Fault::Short(n) = self.faults.remove(at) else {
                unreachable!()
            };
            body.truncate(body.len() - n);
        }
        if let Some(at) = self
            .faults
            .iter()
            .position(|f| matches!(f, Fault::CutOnce(n) if (first..=last).contains(n)))
        {
            let Fault::CutOnce(n) = self.faults.remove(at) else {
                unreachable!()
            };
            sink(&body[..n - first])?;
            return Err(HttpError::Truncated);
        }
        sink(&body)?;
        Ok(Answer::PARTIAL)
    }
}

/// The world a stage runs in.
struct World {
    slot: Mem,
    memo: Memo,
    var: Var,
    nv: Nv,
}

impl World {
    fn new(r: &Release7) -> Self {
        Self {
            slot: Mem::new(r.bytes.len() + 4096),
            memo: Memo::default(),
            var: Var::default(),
            nv: Nv {
                record: record(SEQ - 1),
                one_shot: Some(Slot::A),
            },
        }
    }

    fn stage(&mut self, r: &Release7, server: &mut Server) -> Result<crate::stage::Staged, Stop> {
        let mut partial = SlotPartial::new(&mut self.slot, &mut self.memo, &r.job.release);
        stage(
            &r.job,
            &r.list,
            &mut self.nv,
            &mut self.var,
            &mut partial,
            &mut signed(r),
            &mut |range, sink| server.fetch(range, sink),
        )
    }

    fn written(&self, r: &Release7) -> &[u8] {
        &self.slot.bytes[..r.bytes.len()]
    }
}
