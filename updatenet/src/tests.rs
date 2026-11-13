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

mod recheck;

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
    let record = record(SEQ - 1);
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

// ---- the stage

#[test]
fn a_release_streams_in_chunk_by_chunk_and_becomes_a_trial_only_at_the_end() {
    let r = release(3, 10);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!((staged.slot, staged.release_seq), (Slot::B, SEQ));
    assert_eq!((staged.report.kept, staged.report.fetched), (0, 4));
    assert_eq!(world.written(&r), &r.bytes[..]);
    // The slot was marked Updating, and that was written before the first byte
    // reached it; the trial state is the last write.
    assert_eq!(world.var.sets.len(), 2);
    let first = world.var.sets[0].record.slot(Slot::B);
    assert_eq!((first.reason, first.priority), (Reason::Updating, 0));
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
    let last = world.var.sets[1].record.slot(Slot::B);
    assert_eq!(
        (last.reason, last.release_seq, last.tries, last.successful),
        (Reason::None, SEQ, bootslot::TRIES, false)
    );
    assert!(last.priority > world.nv.record.slot(Slot::A).priority);
    assert!(bootslot::may_boot(&world.var.sets[1].record, Slot::B));
    // Every chunk was recorded, and the last mark is the whole.
    assert_eq!(world.memo.mark.map(|m| m.done), Some(4));
    // The active slot's record was never touched.
    assert_eq!(world.nv.record.slot(Slot::A), record(SEQ - 1).slot(Slot::A));
}

#[test]
fn a_cut_stops_with_a_retry_and_the_next_call_resumes_at_the_first_unchecked_chunk() {
    let r = release(3, 11);
    let mut world = World::new(&r);
    let cut = 2 * CHUNK as usize + 100;
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(cut));
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert!(
        matches!(stop, Stop::Retry { after_s: None, .. }),
        "{stop:?}"
    );
    // Two chunks were checked and written; the slot is still Updating.
    assert_eq!(world.memo.mark.map(|m| m.done), Some(2));
    assert!(!bootslot::may_boot(&world.nv.record, Slot::B));
    assert_eq!(world.var.sets.len(), 1);
    assert_eq!(
        world.var.sets[0].record.slot(Slot::B).reason,
        Reason::Updating
    );
    // Resume: the two kept chunks are read back and checked, two are fetched.
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (2, 2));
    assert_eq!(world.written(&r), &r.bytes[..]);
    // Only the cut chunk and the rest were fetched again.
    let from_chunk_2 = server
        .served
        .iter()
        .filter(|(first, _)| *first >= 2 * CHUNK)
        .count();
    assert_eq!(from_chunk_2, 3, "chunk 2 twice, chunk 3 once");
    assert_eq!(
        server.served.iter().filter(|(f, _)| *f < 2 * CHUNK).count(),
        2
    );
}

#[test]
fn a_resume_after_a_reboot_starts_from_the_mark_with_a_fresh_stage_and_server() {
    let r = release(3, 12);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(CHUNK as usize + 5));
    assert!(world.stage(&r, &mut server).is_err());
    // A reboot forgets everything but the slot, the mark and the boot variable.
    let mut again = World {
        slot: world.slot,
        memo: world.memo,
        var: Var::default(),
        nv: world.var.sets[0],
    };
    let mut fresh = Server::new(&r.bytes);
    let staged = again.stage(&r, &mut fresh).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (1, 3));
    assert_eq!(again.written(&r), &r.bytes[..]);
}

#[test]
fn a_bad_chunk_is_refused_before_a_byte_of_it_is_written_and_the_slot_stays_unbootable() {
    let r = release(3, 13);
    let mut world = World::new(&r);
    let flip = 2 * CHUNK as usize + 9;
    let mut server = Server::new(&r.bytes).with(Fault::Flip(flip));
    assert_eq!(world.stage(&r, &mut server), Err(Stop::Bad(Bad::Chunk(2))));
    // Chunks 0 and 1 were written; nothing of chunk 2 or after.
    assert!(
        world
            .slot
            .writes
            .iter()
            .all(|(offset, _)| *offset < 2 * CHUNK),
        "{:?}",
        world.slot.writes
    );
    assert_eq!(world.memo.mark.map(|m| m.done), Some(2));
    // The boot variable says Updating and nothing after it: it never boots.
    assert_eq!(world.var.sets.len(), 1);
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
    // Asked again with a server that is right, it resumes at chunk 2.
    let mut good = Server::new(&r.bytes);
    let staged = world.stage(&r, &mut good).unwrap();
    assert_eq!(staged.report.kept, 2);
    assert_eq!(world.written(&r), &r.bytes[..]);
}

#[test]
fn a_flipped_byte_in_every_chunk_is_refused_by_that_chunk() {
    let r = release(3, 14);
    for chunk in 0..4 {
        let mut world = World::new(&r);
        let at = chunk * CHUNK as usize + 1;
        let mut server = Server::new(&r.bytes).with(Fault::Flip(at));
        assert_eq!(
            world.stage(&r, &mut server),
            Err(Stop::Bad(Bad::Chunk(chunk))),
            "chunk {chunk}"
        );
        assert_eq!(world.memo.mark.map(|m| m.done).unwrap_or(0), chunk);
        assert_eq!(world.var.sets.len(), 1, "never a trial");
    }
}

#[test]
fn a_chunk_that_comes_short_is_refused_not_resumed() {
    let r = release(2, 15);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::Short(10));
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::Length(0)));
    assert!(world.slot.writes.is_empty());
    assert_eq!(world.var.sets.len(), 1);
}

#[test]
fn a_busy_server_is_a_retry_with_its_wait_and_anything_else_is_a_refusal() {
    let r = release(2, 16);
    for (status, after, want) in [
        (429, Some(30), Some(Some(30))),
        (503, None, Some(None)),
        (500, Some(5), Some(Some(5))),
        (404, None, None),
        (403, None, None),
        (200, None, None),
        (302, None, None),
    ] {
        let mut world = World::new(&r);
        let mut server = Server::new(&r.bytes).with(Fault::Status(status, after));
        let stop = world.stage(&r, &mut server).unwrap_err();
        match want {
            Some(wait) => assert!(
                matches!(&stop, Stop::Retry { after_s, .. } if *after_s == wait),
                "{status}: {stop:?}"
            ),
            None => assert_eq!(stop, Stop::Bad(Bad::Status(status)), "{status}"),
        }
        assert!(world.slot.writes.is_empty(), "{status}");
        assert_eq!(world.var.sets.len(), 1);
    }
}

#[test]
fn a_certificate_or_host_refusal_is_a_refusal_and_a_dropped_connection_is_a_retry() {
    let r = release(1, 17);
    let mut world = World::new(&r);
    let mut refused = |_: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        Err(HttpError::Refused(String::from("elsewhere.example")))
    };
    let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
    let stop = stage(
        &r.job,
        &r.list,
        &mut world.nv,
        &mut world.var,
        &mut partial,
        &mut signed(&r),
        &mut refused,
    )
    .unwrap_err();
    assert!(matches!(stop, Stop::Bad(Bad::Fetch(_))), "{stop:?}");
    for error in [
        HttpError::Truncated,
        HttpError::Timeout,
        HttpError::Connect("no route"),
    ] {
        let mut world = World::new(&r);
        let mut fails =
            |_: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| Err(error.clone());
        let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
        let stop = stage(
            &r.job,
            &r.list,
            &mut world.nv,
            &mut world.var,
            &mut partial,
            &mut signed(&r),
            &mut fails,
        )
        .unwrap_err();
        assert!(matches!(stop, Stop::Retry { .. }), "{error:?}: {stop:?}");
    }
}

#[test]
fn a_list_whose_chunks_do_not_make_the_signed_image_forgets_the_mark() {
    let r = release(2, 18);
    // Every chunk hash right, the whole wrong: a list built wrong, as signed.
    let mut list = r.list.clone();
    list.blob = [9; 32];
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes);
    let mut partial = SlotPartial::new(&mut world.slot, &mut world.memo, &r.job.release);
    let stop = stage(
        &r.job,
        &list,
        &mut world.nv,
        &mut world.var,
        &mut partial,
        &mut signed(&r),
        &mut |range, sink| server.fetch(range, sink),
    )
    .unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::Whole));
    assert_eq!(world.memo.mark, None, "nothing written under it resumes");
    assert_eq!(world.var.sets.len(), 1, "never a trial");
}

#[test]
fn a_slot_that_reads_back_other_than_it_was_written_is_not_staged() {
    let r = release(2, 19);
    let mut world = World::new(&r);
    // The write of chunk 1 is dropped, as a bad sector drops it, though the slot
    // says it was written and the mark records it.
    world.slot.drop_at = Some(CHUNK);
    let mut server = Server::new(&r.bytes);
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Bad(Bad::ReadBack));
    assert_eq!(world.var.sets.len(), 1, "never a trial");
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
}

#[test]
fn a_chunk_that_rotted_on_disk_between_runs_is_fetched_again() {
    let r = release(3, 20);
    let mut world = World::new(&r);
    let mut server = Server::new(&r.bytes).with(Fault::CutOnce(3 * CHUNK as usize));
    assert!(world.stage(&r, &mut server).is_err());
    assert_eq!(world.memo.mark.map(|m| m.done), Some(3));
    // Chunk 1 rots in the slot while the machine is off.
    world.slot.bytes[CHUNK as usize + 3] ^= 0x40;
    let staged = world.stage(&r, &mut server).unwrap();
    assert_eq!(staged.report.kept, 1, "only chunk 0 is still right");
    assert_eq!(world.written(&r), &r.bytes[..]);
}

#[test]
fn a_mark_for_another_release_starts_the_download_over() {
    let old = release(2, 21);
    let new = release(2, 22);
    assert_ne!(old.job.release.sha256, new.job.release.sha256);
    let mut world = World::new(&old);
    let mut server = Server::new(&old.bytes).with(Fault::CutOnce(CHUNK as usize + 1));
    assert!(world.stage(&old, &mut server).is_err());
    assert_eq!(world.memo.mark.map(|m| m.done), Some(1));
    // A newer release was published: the old mark is dropped, the slot rewritten.
    let mut fresh = Server::new(&new.bytes);
    let staged = world.stage(&new, &mut fresh).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (0, 3));
    assert_eq!(world.written(&new), &new.bytes[..]);
    assert_eq!(
        world.memo.mark.map(|m| m.sha256),
        Some(new.job.release.sha256)
    );
}

#[test]
fn a_boot_variable_that_cannot_be_written_stops_before_the_slot_is_touched() {
    let r = release(1, 23);
    let mut world = World::new(&r);
    world.var.fail_nth = Some(1);
    let mut server = Server::new(&r.bytes);
    let stop = world.stage(&r, &mut server).unwrap_err();
    assert_eq!(stop, Stop::Store("the boot variable cannot be written"));
    assert!(world.slot.writes.is_empty());
    assert!(server.served.is_empty());

    // The last write failing leaves the variable as Updating: no trial.
    let mut world = World::new(&r);
    world.var.fail_nth = Some(2);
    let stop = world.stage(&r, &mut Server::new(&r.bytes)).unwrap_err();
    assert!(matches!(stop, Stop::Store(_)));
    assert_eq!(world.var.sets.len(), 1);
    assert!(!bootslot::may_boot(&world.var.sets[0].record, Slot::B));
}

#[test]
fn the_slot_is_flushed_before_a_mark_says_a_chunk_is_in_it() {
    // A slot whose flush fails: the mark must not advance past what it holds.
    struct Unflushable(Mem);
    impl SlotIo for Unflushable {
        fn write(&mut self, o: u64, b: &[u8]) -> Result<(), &'static str> {
            self.0.write(o, b)
        }
        fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), &'static str> {
            self.0.read(o, b)
        }
        fn flush(&mut self) -> Result<(), &'static str> {
            Err("the slot cannot be flushed")
        }
    }
    let r = release(1, 24);
    let mut slot = Unflushable(Mem::new(r.bytes.len() + 4096));
    let mut memo = Memo::default();
    let mut var = Var::default();
    let mut nv = World::new(&r).nv;
    let mut server = Server::new(&r.bytes);
    let mut partial = SlotPartial::new(&mut slot, &mut memo, &r.job.release);
    let stop = stage(
        &r.job,
        &r.list,
        &mut nv,
        &mut var,
        &mut partial,
        &mut signed(&r),
        &mut |range, sink| server.fetch(range, sink),
    )
    .unwrap_err();
    assert_eq!(stop, Stop::Store("the slot cannot be flushed"));
    assert_eq!(memo.mark, None);
}

#[test]
fn every_stop_has_words() {
    let stops = [
        Stop::Retry {
            after_s: Some(30),
            why: String::from("the server answered 429"),
        },
        Stop::Retry {
            after_s: None,
            why: String::from("Truncated"),
        },
        Stop::Bad(Bad::Chunk(3)),
        Stop::Bad(Bad::Length(1)),
        Stop::Bad(Bad::Whole),
        Stop::Bad(Bad::ReadBack),
        Stop::Bad(Bad::Status(404)),
        Stop::Bad(Bad::Fetch(String::from("Refused"))),
        Stop::Store("the slot cannot be flushed"),
    ];
    for stop in stops {
        let words = stop.why();
        assert!(!words.is_empty(), "{stop:?}");
    }
    assert!(Stop::Bad(Bad::Chunk(3)).why().contains("part 3"));
    assert!(
        Stop::Retry {
            after_s: Some(30),
            why: String::new()
        }
        .why()
        .contains("after 30 s")
    );
    assert_eq!(READ_BACK, "the slot reads back other than the signed image");
    for refused in [
        Refused::NotSigned("slot.img"),
        Refused::Repository(String::from("expired")),
        Refused::Field("x"),
        Refused::Rollback { seq: 1, newest: 2 },
        Refused::Slot("x"),
        Refused::Room { need: 2, room: 1 },
        Refused::Staged(1),
        Refused::List("x"),
    ] {
        assert!(!refused.why().is_empty());
    }
}
