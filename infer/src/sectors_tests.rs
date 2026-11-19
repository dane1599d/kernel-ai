//! [`SectorRegion`] on a disk in memory that answers as the OS's spill region
//! does: whole sectors, a bounded call, NotFound past the end. The size is found
//! once at open; after that a call past the end is a failure like any other; a
//! spill on it comes back as a fresh slot answers.

extern crate std;

use crate::chat::{Engine, Role, Turn};
use crate::fixture::{self, Spec};
use crate::kvstore::{KvStore, MAX_POOL_BYTES, Place, Region, Unkept};
use crate::sectors::{Miss, SectorRegion, Sectored};
use crate::segments_tests::{ROOMY, key, later, reply, saved};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use quant::Tier;

const SECTOR: u64 = 512;
/// `kabi::model::KV_IO_BYTES`.
const IO: u64 = 512 << 10;

/// What a disk call did: (load or save, at, length, flush).
type Call = (&'static str, u64, u64, bool);

/// A disk of `size` bytes keeping only the sectors written, so a large one costs
/// nothing. It can fail every call or shrink under an open region, and it
/// records the calls.
#[derive(Default)]
struct State {
    size: u64,
    sectors: BTreeMap<u64, Vec<u8>>,
    fail: bool,
    calls: Vec<Call>,
}

/// A handle to it: the region holds one, the test another.
#[derive(Clone)]
struct Disk(Rc<RefCell<State>>);

impl Disk {
    fn new(size: u64) -> Self {
        Self(Rc::new(RefCell::new(State {
            size,
            ..State::default()
        })))
    }

    fn calls(&self) -> Vec<Call> {
        self.0.borrow().calls.clone()
    }

    /// As the OS checks a call: whole sectors, at most [`IO`], inside the area.
    fn check(state: &State, at: u64, len: usize) -> Result<(), Miss> {
        let len = len as u64;
        assert!(at.is_multiple_of(SECTOR) && len.is_multiple_of(SECTOR) && len > 0);
        assert!(len <= IO, "a call of {len} bytes");
        match (state.fail, at + len <= state.size) {
            (true, _) => Err(Miss::Failed),
            (false, true) => Ok(()),
            (false, false) => Err(Miss::PastEnd),
        }
    }
}

impl Sectored for Disk {
    fn load(&mut self, at: u64, buf: &mut [u8]) -> Result<(), Miss> {
        let mut state = self.0.borrow_mut();
        state.calls.push(("load", at, buf.len() as u64, false));
        Self::check(&state, at, buf.len())?;
        for (i, sector) in buf.chunks_mut(SECTOR as usize).enumerate() {
            match state.sectors.get(&(at / SECTOR + i as u64)) {
                Some(bytes) => sector.copy_from_slice(bytes),
                None => sector.fill(0),
            }
        }
        Ok(())
    }

    fn save(&mut self, at: u64, bytes: &[u8], flush: bool) -> Result<(), Miss> {
        let mut state = self.0.borrow_mut();
        state.calls.push(("save", at, bytes.len() as u64, flush));
        Self::check(&state, at, bytes.len())?;
        for (i, sector) in bytes.chunks(SECTOR as usize).enumerate() {
            state
                .sectors
                .insert(at / SECTOR + i as u64, sector.to_vec());
        }
        Ok(())
    }
}

fn open(disk: &Disk) -> Option<SectorRegion<Disk>> {
    SectorRegion::open(disk.clone(), SECTOR, IO)
}

#[test]
fn open_finds_the_size_in_a_few_single_sector_loads() {
    const MIB: u64 = 1 << 20;
    // The default 1 GiB disk's 384 MiB, a part-filled power of two, one sector.
    for size in [384 * MIB, 3 * SECTOR, 64 * SECTOR, SECTOR] {
        let disk = Disk::new(size);
        let region = open(&disk).unwrap();
        assert_eq!(region.size(), size);
        assert_eq!(region.block(), SECTOR);
        let calls = disk.calls();
        assert!(calls.len() <= 45, "{} loads for {size}", calls.len());
        assert!(calls.iter().all(|c| c.0 == "load" && c.2 == SECTOR));
    }
}

#[test]
fn no_room_at_the_first_sector_or_a_failed_probe_is_no_region() {
    // No spill room past the slot regions (or no kai-cache): nothing to open.
    assert!(open(&Disk::new(0)).is_none());
    // A probe that fails for another reason gives no size to trust.
    let failing = Disk::new(4 * SECTOR);
    failing.0.borrow_mut().fail = true;
    assert!(open(&failing).is_none());
    // Sizes it cannot work in.
    for (sector, io) in [(0, IO), (SECTOR, SECTOR - 1), (SECTOR, IO + 1)] {
        assert!(SectorRegion::open(Disk::new(IO), sector, io).is_none());
    }
}

#[test]
fn reads_and_writes_go_in_whole_bounded_calls_and_round_trip() {
    let disk = Disk::new(4 * IO);
    let mut region = open(&disk).unwrap();
    // Two and a half calls' worth, from the second sector.
    let len = (2 * IO + IO / 2) as usize;
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    assert!(region.write_at(SECTOR, &bytes));
    let mut back = vec![0u8; len];
    assert!(region.read_at(SECTOR, &mut back));
    assert_eq!(back, bytes);
    let saves: Vec<Call> = disk.calls().into_iter().filter(|c| c.0 == "save").collect();
    assert_eq!(
        saves,
        [
            ("save", SECTOR, IO, false),
            ("save", SECTOR + IO, IO, false),
            ("save", SECTOR + 2 * IO, IO / 2, false),
        ]
    );
}

#[test]
fn flush_saves_the_last_sector_written_again_with_the_flush() {
    let disk = Disk::new(IO);
    let mut region = open(&disk).unwrap();
    // Nothing written: nothing to make durable.
    assert!(region.flush());
    let bytes: Vec<u8> = (0..3 * SECTOR).map(|i| i as u8).collect();
    assert!(region.write_at(2 * SECTOR, &bytes));
    assert!(region.flush());
    assert_eq!(
        disk.calls().last(),
        Some(&("save", 4 * SECTOR, SECTOR, true))
    );
    // Saved again with the bytes it had.
    assert_eq!(disk.0.borrow().sectors[&4], bytes[2 * SECTOR as usize..]);
}

/// K's note on #1076: after open, a call past the end is a failed call. The
/// region keeps the size it found, and still works where it can.
#[test]
fn past_the_end_after_open_is_a_failed_call_not_a_lost_region() {
    let disk = Disk::new(4 * SECTOR);
    let mut region = open(&disk).unwrap();
    let mut two = vec![0u8; 2 * SECTOR as usize];
    // Outside the size found: refused before any disk call.
    let before = disk.calls().len();
    assert!(!region.read_at(3 * SECTOR, &mut two));
    assert!(!region.write_at(3 * SECTOR, &two));
    // Part of a sector, or off a sector's start.
    assert!(!region.write_at(1, &two));
    assert!(!region.read_at(0, &mut two[..100]));
    assert_eq!(disk.calls().len(), before);
    // The disk answers past-the-end inside the size found: that call fails, the
    // region does not shrink, and a call that fits still goes through.
    disk.0.borrow_mut().size = 2 * SECTOR;
    assert!(!region.write_at(2 * SECTOR, &two));
    assert_eq!(region.size(), 4 * SECTOR);
    assert!(region.write_at(0, &two));
    assert!(region.read_at(0, &mut two));
    // A disk that fails outright fails the flush too.
    disk.0.borrow_mut().fail = true;
    assert!(!region.flush());
}

fn engine(bytes: &[u8]) -> Engine<'_> {
    Engine::load(bytes, Tier::best(), 256, ROOMY).unwrap()
}

/// The store spills to it and the state comes back as a fresh slot answers;
/// with no room at open there is no region, and the spill is NoRoom.
#[test]
fn a_spill_on_it_comes_back_as_a_fresh_slot() {
    let bytes = fixture::build(&Spec::tiny());
    let k = key(Tier::best());
    let mut e = engine(&bytes);
    reply(&mut e, &saved(), 1);
    let file = e.slot_file_bytes() as u64;
    let disk = Disk::new(file.div_ceil(SECTOR) * SECTOR);
    let mut store = KvStore::new(MAX_POOL_BYTES, open(&disk));
    store.park(&e, &k, 1, Place::Spill).unwrap();
    assert!(
        disk.calls().last().is_some_and(|c| c.3),
        "the spill flushed"
    );
    reply(&mut e, &[Turn::new(Role::User, "something else")], 4);
    let n = store.restore(&mut e, &k, 1, &later()).unwrap();
    assert!(n > 0);
    let got = reply(&mut e, &later(), 12);
    let want = reply(&mut engine(&bytes), &later(), 12);
    assert_eq!(got.cached, n);
    assert_eq!((got.bytes, got.tokens), (want.bytes, want.tokens));

    let mut none = KvStore::new(MAX_POOL_BYTES, open(&Disk::new(0)));
    assert_eq!(none.park(&e, &k, 1, Place::Spill), Err(Unkept::NoRoom));
}
