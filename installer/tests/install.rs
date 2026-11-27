//! The installer's plan over in-memory disks: the installed disk reads back
//! through `diskmap::read` and kaifs mounts its `kai-data`; installs get their
//! own GUIDs; and a power cut at any point, with any of the writes since the
//! last flush lost, leaves either the old install whole or a disk with no valid
//! GPT, until the backup header lands on a whole new one.

mod common;

use std::collections::BTreeMap;

use common::*;
use diskmap::{Guid, KAI_A_TYPE, KAI_B_TYPE, Table};
use installer::{
    CLEARED, Committed, Current, Error, Expect, FOLDERS, Found, PARTITION_MIB, Plan, Source, Step,
    Target, Window, apply, current, minimum_sectors, plan,
};
use kaifs::Fs;

fn target(id: &str) -> Target<'_> {
    Target {
        sectors: disk_sectors(),
        sector_size: 512,
        id,
        now: 1_790_000_000,
    }
}

/// `plan` for `id` from `running`, applied to `disk`.
fn install(disk: &mut Mem, running: &Running, id: &str) -> Plan {
    let now = current(disk);
    let plan = plan(target(id), running.source(), &now).unwrap();
    apply(&plan, &mut running.disk.clone(), disk).unwrap();
    plan
}

/// Checks that `disk` is a whole install of `plan` from `running`.
fn installed(disk: &Mem, plan: &Plan, running: &Running) {
    let mut disk = disk.clone();
    let table = diskmap::read(&mut disk).unwrap();
    assert_eq!(table.disk_guid, plan.disk_guid);
    let got: Vec<_> = table
        .partitions
        .iter()
        .map(|p| (p.kind, p.guid, p.first, p.last, p.name.clone()))
        .collect();
    let want: Vec<_> = plan
        .parts
        .iter()
        .map(|p| (p.kind, p.guid, p.first, p.last, p.name.to_string()))
        .collect();
    assert_eq!(got, want);
    let from = &running.table;
    let slot = from.find(running.committed.kind).unwrap().first;
    let sources = [from.partitions[0].first, from.partitions[1].first, slot];
    for (n, at) in sources.into_iter().enumerate() {
        let to = plan.parts[n].first;
        let name = plan.parts[n].name;
        assert_eq!(
            disk.hash(to, VOLUME),
            running.disk.hash(at, VOLUME),
            "{name}"
        );
        assert!(
            disk.get(to + VOLUME) != [0xEE; S],
            "past {name}'s volume copied"
        );
    }
    let b = &plan.parts[3];
    assert!((b.first..b.first + CLEARED).all(|lba| disk.get(lba) == [0; S]));
    let data = &plan.parts[4];
    assert!((data.first..data.first + 8).all(|lba| disk.get(lba) == [0; S]));
    mounts(&mut disk, data.first, data.sectors());
}

fn mounts(disk: &mut Mem, first: u64, sectors: u64) {
    let window = Window {
        disk,
        first,
        blocks: sectors / 8,
    };
    let mut fs = Fs::mount(window).expect("kai-data mounts");
    for (vol, path) in FOLDERS {
        fs.set_volume(vol).unwrap();
        fs.resolve(path).unwrap_or_else(|e| panic!("{path}: {e:?}"));
    }
}

#[test]
fn an_install_reads_back_and_its_data_mounts() {
    for slot in [KAI_A_TYPE, KAI_B_TYPE] {
        let running = Running::new(7, slot);
        let mut disk = Mem::new(disk_sectors());
        let plan = install(&mut disk, &running, "install-1");
        assert_eq!(plan.wipes, Current::Blank);
        installed(&disk.cut(), &plan, &running);
        assert!(!diskmap::read(&mut disk.cut()).unwrap().from_backup);
        // ADR 0008 §1: 2,368 MiB before kai-data, which starts on a MiB.
        assert_eq!(plan.parts[4].first, 2048 * (1 + 2368));
        assert_eq!(plan.parts[4].sectors() % 8, 0);
        let sizes: Vec<u64> = plan.parts[..4].iter().map(|p| p.sectors() / 2048).collect();
        assert_eq!(sizes, PARTITION_MIB);
    }
}

#[test]
fn two_installs_share_no_guid_and_one_id_writes_the_same_disk() {
    let running = Running::new(7, KAI_B_TYPE);
    let (mut a, mut again, mut b) = (
        Mem::new(disk_sectors()),
        Mem::new(disk_sectors()),
        Mem::new(disk_sectors()),
    );
    let (pa, _, pb) = (
        install(&mut a, &running, "install-a"),
        install(&mut again, &running, "install-a"),
        install(&mut b, &running, "install-b"),
    );
    assert!(a.durable == again.durable, "one id wrote two disks");
    let guids: Vec<Guid> = [&pa, &pb]
        .iter()
        .flat_map(|p| p.parts.iter().map(|x| x.guid).chain([p.disk_guid]))
        .collect();
    for (i, g) in guids.iter().enumerate() {
        assert!(!guids[i + 1..].contains(g), "a GUID repeats");
        assert!(!diskmap::TYPES.contains(g));
    }
    let uuid = |p: &Plan| {
        p.steps.iter().find_map(|s| match s {
            Step::Format { uuid, .. } => Some(*uuid),
            _ => None,
        })
    };
    assert_ne!(uuid(&pa), uuid(&pb));
    assert_eq!(pa.disk_guid, Guid(installer::guid("install-a disk")));
}

/// The sectors that decide what `diskmap::read` finds: the MBR, the primary
/// header and array, and the backup array and header.
fn decides(lba: u64, n: usize) -> bool {
    let tail = disk_sectors() - 1 - diskmap::ENTRY_SECTORS;
    lba < diskmap::FIRST_USABLE || lba + n as u64 > tail
}

/// `durable` with `writes` on top, in order, all durable.
fn with(durable: &BTreeMap<u64, Sector>, writes: &[&(u64, Vec<Sector>)]) -> Mem {
    let mut disk = Mem::new(disk_sectors());
    disk.durable = durable.clone();
    for (lba, sectors) in writes {
        for (i, s) in sectors.iter().enumerate() {
            set(&mut disk.durable, lba + i as u64, *s);
        }
    }
    disk
}

/// Over an old install, a cut at every write keeping any subset of the writes
/// since the last flush: the old install whole, or no valid GPT, until the
/// backup header lands on a whole new install. A valid new primary header
/// never stands without a valid new backup.
#[test]
fn a_cut_anywhere_never_leaves_a_half_install_that_boots() {
    let old_running = Running::new(3, KAI_A_TYPE);
    let mut old = Mem::new(disk_sectors());
    let old_plan = install(&mut old, &old_running, "old");
    let mut old = old.cut();
    // What an old install's use leaves: a slot volume in kai-b, and a
    // `kai-modelhint 1` in kai-data's block 0. The install must clear both.
    let slot = old_plan.parts[2].first;
    old.put(old_plan.parts[3].first, old.get(slot));
    let mut hint = [0u8; S];
    let text = b"kai-modelhint 1\nblob old-model\n";
    hint[..text.len()].copy_from_slice(text);
    old.put(old_plan.parts[4].first, hint);
    let old_table = current(&mut old.clone()).gpt().unwrap();
    let old_whole = |disk: &Mem| {
        for p in &old_plan.parts[..3] {
            let name = p.name;
            assert_eq!(
                disk.hash(p.first, VOLUME),
                old.hash(p.first, VOLUME),
                "{name}"
            );
        }
        let data = &old_plan.parts[4];
        mounts(&mut disk.clone(), data.first, data.sectors());
    };

    let running = Running::new(7, KAI_B_TYPE);
    let mut logged = old.clone();
    let plan = install(&mut logged, &running, "new");
    let mut durable = old.durable.clone();
    let mut pending: Vec<&(u64, Vec<Sector>)> = Vec::new();
    let (mut olds, mut news) = (0, 0);
    for op in &logged.log {
        let Some(write) = op else {
            for (lba, sectors) in pending.drain(..) {
                for (i, s) in sectors.iter().enumerate() {
                    set(&mut durable, lba + i as u64, *s);
                }
            }
            continue;
        };
        pending.push(write);
        // Every subset of the pending writes that decide the table, each with
        // all or none of the rest.
        let (table, rest): (Vec<_>, Vec<_>) = pending
            .iter()
            .copied()
            .partition(|(lba, s)| decides(*lba, s.len()));
        assert!(
            table.len() <= 4,
            "{} table writes in one flush",
            table.len()
        );
        for mask in 0..1u32 << table.len() {
            let kept: Vec<_> = (0..table.len())
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| table[i])
                .collect();
            let mut disk = with(&durable, &kept);
            let Ok(found) = diskmap::read(&mut disk) else {
                continue;
            };
            for others in [&[][..], &rest[..]] {
                let state = with(&disk.durable, others);
                // Read from either copy: the old primary may be zeroed first.
                if (found.disk_guid, &found.partitions)
                    == (old_table.disk_guid, &old_table.partitions)
                {
                    olds += 1;
                    old_whole(&state);
                } else {
                    news += 1;
                    assert_eq!(found.disk_guid, plan.disk_guid, "a third table");
                    installed(&state, &plan, &running);
                    backed(&disk, &found);
                }
            }
        }
    }
    assert!(olds > 0 && news > 0, "old {olds}, new {news}");
    installed(&logged.cut(), &plan, &running);
}

/// A new table read from the primary has a valid backup too: the primary
/// header is written last, in a flush of its own.
fn backed(disk: &Mem, found: &Table) {
    if found.from_backup {
        return;
    }
    let mut backup = disk.clone();
    backup.put(1, [0; S]);
    let got = diskmap::read(&mut backup).expect("a primary header without a backup");
    assert!(got.from_backup);
    assert_eq!(got.disk_guid, found.disk_guid);
    assert_eq!(got.partitions, found.partitions);
}

#[test]
fn a_disk_too_small_or_not_512_byte_sectors_is_refused() {
    let running = Running::new(7, KAI_A_TYPE);
    let at = |sectors: u64, sector_size: u32| {
        let t = Target {
            sectors,
            sector_size,
            id: "x",
            now: 0,
        };
        plan(t, running.source(), &Current::Blank)
    };
    let need = minimum_sectors();
    assert_eq!(at(need - 1, 512), Err(Error::TooSmall { need }));
    assert_eq!(at(disk_sectors(), 4096), Err(Error::SectorSize(4096)));
    // 2,368 MiB and kai-data's minimum: it installs, and kai-data mounts.
    let plan = at(need, 512).unwrap();
    assert_eq!(plan.parts[4].sectors(), installer::DATA_MIN_BLOCKS * 8);
    let mut disk = Mem::new(need);
    apply(&plan, &mut running.disk.clone(), &mut disk).unwrap();
    let data = &plan.parts[4];
    mounts(&mut disk, data.first, data.sectors());
    // A target not the plan's size is refused before any write.
    let mut other = Mem::new(need + 8);
    let refused = apply(&plan, &mut running.disk.clone(), &mut other);
    assert_eq!(refused, Err(Error::Target));
    assert!(other.log.is_empty());
}

#[test]
fn bad_ids_are_refused() {
    let running = Running::new(7, KAI_A_TYPE);
    for id in ["", "has space", &"x".repeat(65), "tab\t"] {
        let got = plan(target(id), running.source(), &Current::Blank);
        assert_eq!(got, Err(Error::BadId), "{id:?}");
    }
}

/// The plan names the table it wipes; the running disk is refused, and so is
/// a target whose table changed since the plan was made.
#[test]
fn the_plan_names_what_it_wipes_and_refuses_the_running_disk() {
    let running = Running::new(7, KAI_A_TYPE);
    let on_itself = plan(
        target("x"),
        running.source(),
        &Current::Gpt(running.table.clone()),
    );
    assert_eq!(on_itself, Err(Error::RunningDisk));
    let mut disk = Mem::new(disk_sectors());
    let first = install(&mut disk, &running, "first");
    let held = current(&mut disk);
    assert_eq!(held.clone().gpt().unwrap().disk_guid, first.disk_guid);
    let again = plan(target("second"), running.source(), &held).unwrap();
    assert_eq!(again.wipes, held);
    // Planned over that table, applied to a blank disk: refused, unwritten.
    let mut blank = Mem::new(disk_sectors());
    let refused = apply(&again, &mut running.disk.clone(), &mut blank);
    assert_eq!(refused, Err(Error::Target));
    assert!(blank.log.is_empty());
    // Planned over nothing, applied over the first install: the same.
    let blind = plan(target("second"), running.source(), &Current::Blank).unwrap();
    disk.log.clear();
    let refused = apply(&blind, &mut running.disk.clone(), &mut disk);
    assert_eq!(refused, Err(Error::Target));
    assert!(disk.log.is_empty());
    apply(&again, &mut running.disk.clone(), &mut disk).unwrap();
    installed(&disk.cut(), &again, &running);
}

fn source<'a>(table: &'a Table, committed: Committed) -> Source<'a> {
    Source { table, committed }
}

/// A running disk that cannot be copied is refused, and nothing is written.
#[test]
fn a_running_disk_that_cannot_be_copied_writes_nothing() {
    let running = Running::new(7, KAI_A_TYPE);
    let committed = running.committed;
    let mut missing = running.table.clone();
    missing.partitions.retain(|p| p.kind != diskmap::ESP_TYPE);
    let mut larger = running.table.clone();
    larger.partitions[1].last += 1;
    let mut not_a_slot = committed;
    not_a_slot.kind = diskmap::KAI_R_TYPE;
    for (why, src) in [
        ("no ESP", source(&missing, committed)),
        ("a larger kai-r", source(&larger, committed)),
        ("R as the slot", source(&running.table, not_a_slot)),
    ] {
        let got = plan(target("x"), src, &Current::Blank);
        assert!(matches!(got, Err(Error::Source(_))), "{why}: {got:?}");
    }

    let plan = plan(target("x"), running.source(), &Current::Blank).unwrap();
    let r = running.table.partitions[1].first;
    let mut past = running.disk.get(r);
    past[32..36].copy_from_slice(&(256 * 2048 + 1u32).to_le_bytes());
    let mut fat16 = running.disk.get(r);
    fat16[19..21].copy_from_slice(&100u16.to_le_bytes());
    for (why, boot) in [
        ("no FAT volume", [0u8; S]),
        ("a volume past its partition", past),
        ("FAT16 fields", fat16),
    ] {
        let mut source = running.disk.clone();
        source.put(r, boot);
        let mut disk = Mem::new(disk_sectors());
        let got = apply(&plan, &mut source, &mut disk);
        assert!(matches!(got, Err(Error::Source(_))), "{why}: {got:?}");
        assert!(disk.log.is_empty(), "{why}: wrote");
    }
}

/// `plan` with the committed slot's expectation replaced.
fn expecting(plan: &Plan, expect: Expect) -> Plan {
    let mut p = plan.clone();
    for step in &mut p.steps {
        if let Step::Check { expect: e, .. } = step
            && matches!(e, Expect::Slot { .. })
        {
            *e = expect;
        }
    }
    p
}

/// Whether no table write landed: every write to the sectors that decide the
/// table wrote zeros.
fn no_table(disk: &Mem) -> bool {
    disk.writes()
        .all(|(lba, s)| !decides(*lba, s.len()) || s.iter().all(|x| *x == [0; S]))
}

/// A source whose sector `at` changes at its `change_at`th read.
struct Flaky<'a> {
    disk: &'a mut Mem,
    at: u64,
    reads: u32,
    change_at: u32,
}

impl installer::Disk for Flaky<'_> {
    fn sectors(&self) -> u64 {
        self.disk.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), installer::IoError> {
        let n = (out.len() / S) as u64;
        if (lba..lba + n).contains(&self.at) {
            self.reads += 1;
            if self.reads == self.change_at {
                self.disk.put(self.at, [0x77; S]);
            }
        }
        installer::Disk::read(self.disk, lba, out)
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), installer::IoError> {
        installer::Disk::write(self.disk, lba, data)
    }

    fn flush(&mut self) -> Result<(), installer::IoError> {
        installer::Disk::flush(self.disk)
    }
}

/// A running slot that is not the committed one is refused before any write.
#[test]
fn a_running_slot_that_is_not_the_committed_one_writes_nothing() {
    let running = Running::new(7, KAI_B_TYPE);
    let base = plan(target("x"), running.source(), &Current::Blank).unwrap();
    let Committed {
        sha256,
        release_seq,
        ..
    } = running.committed;
    let mut wrong = sha256;
    wrong[0] ^= 1;
    for (why, expect) in [
        (
            "another SHA-256",
            Expect::Slot {
                sha256: wrong,
                release_seq,
            },
        ),
        (
            "another release",
            Expect::Slot {
                sha256,
                release_seq: release_seq + 1,
            },
        ),
    ] {
        let mut disk = Mem::new(disk_sectors());
        let got = apply(
            &expecting(&base, expect),
            &mut running.disk.clone(),
            &mut disk,
        );
        assert!(matches!(got, Err(Error::Source(_))), "{why}: {got:?}");
        assert!(disk.log.is_empty(), "{why}: wrote");
    }
}

/// A copy that differs from what was checked before the writes stops the
/// install before the table: no GPT is written.
#[test]
fn a_copy_that_does_not_read_back_writes_no_table() {
    let running = Running::new(7, KAI_B_TYPE);
    let base = plan(target("x"), running.source(), &Current::Blank).unwrap();
    let slot = running.table.find(KAI_B_TYPE).unwrap().first;
    // A source sector that changes after its first read: the ESP's (read by
    // the copy, then the read-back), and the slot's (read by the check before
    // the writes, then the copy).
    for (why, at, reads) in [
        ("the ESP", running.table.partitions[0].first + 5000, 2),
        ("the slot", slot + 1200, 2),
    ] {
        let mut flaky = running.disk.clone();
        let mut source = Flaky {
            disk: &mut flaky,
            at,
            reads: 0,
            change_at: reads,
        };
        let mut disk = Mem::new(disk_sectors());
        let got = apply(&base, &mut source, &mut disk);
        assert!(matches!(got, Err(Error::Verify(_))), "{why}: {got:?}");
        assert!(
            matches!(current(&mut disk.cut()), Current::Blank),
            "{why}: a table"
        );
        assert!(no_table(&disk), "{why}: a table write");
    }
}

/// `sector` with `bytes` at `at`.
fn with_bytes(mut sector: Sector, at: usize, bytes: &[u8]) -> Sector {
    sector[at..at + bytes.len()].copy_from_slice(bytes);
    sector
}

/// A blank disk with `sectors` put at their LBAs.
fn holding(sectors: &[(u64, Sector)]) -> Mem {
    let mut disk = Mem::new(disk_sectors());
    for (lba, s) in sectors {
        disk.put(*lba, *s);
    }
    disk
}

fn found(disk: &mut Mem) -> Found {
    match current(disk) {
        Current::Other { found, .. } => found,
        other => panic!("not Other: {other:?}"),
    }
}

/// What a target holds, as the device shows it before it asks to go on:
/// blank only when both GPT areas are zero, and what else is there when it
/// is neither blank nor a GPT.
#[test]
fn the_current_contents_say_blank_gpt_or_what_else() {
    let running = Running::new(7, KAI_A_TYPE);
    let mut disk = Mem::new(disk_sectors());
    assert_eq!(current(&mut disk), Current::Blank);
    install(&mut disk, &running, "x");
    assert!(matches!(current(&mut disk), Current::Gpt(_)));

    // A refused GPT: both arrays fail their checksums.
    let mut bad = disk.cut();
    let tail = disk_sectors() - 1 - diskmap::ENTRY_SECTORS;
    for lba in [2, tail] {
        let s = bad.get(lba);
        bad.put(lba, with_bytes(s, 0, &[s[0] ^ 1]));
    }
    assert_eq!(found(&mut bad), Found::GptHeader);

    let zero = [0u8; S];
    let signed = with_bytes(zero, 510, &[0x55, 0xAA]);
    // A whole-disk LUKS volume: its header at sector 0, no boot signature.
    let mut luks = holding(&[(0, with_bytes(zero, 0, b"LUKS\xba\xbe\x00\x02"))]);
    assert_eq!(found(&mut luks), Found::Luks);
    // A whole-disk ext4 volume: sectors 0 and 1 zero, its superblock at
    // byte 1024 with the magic at 56.
    let mut ext = holding(&[(2, with_bytes(zero, 56, &[0x53, 0xEF]))]);
    assert_eq!(found(&mut ext), Found::Ext);
    // An MBR stick with one FAT32 partition (type 0x0C).
    let mbr = with_bytes(signed, 446 + 4, &[0x0C]);
    let fat = with_bytes(
        with_bytes(signed, 82, b"FAT32   "),
        11,
        &512u16.to_le_bytes(),
    );
    let mut stick = holding(&[(0, mbr), (2048, fat)]);
    assert_eq!(found(&mut stick), Found::Mbr([0x0C, 0, 0, 0]));
    // A whole-disk FAT32 volume, no MBR.
    let mut floppy = holding(&[(0, fat)]);
    assert_eq!(found(&mut floppy), Found::Fat);
    // Bytes only in the backup GPT area.
    let mut tail_only = holding(&[(disk_sectors() - 1, [0x11; S])]);
    assert_eq!(found(&mut tail_only), Found::Unknown);

    // Planned over one LUKS disk: applied to a blank disk, to an ext4 disk,
    // or to another LUKS disk of the same size, refused and unwritten.
    let over = plan(target("y"), running.source(), &current(&mut luks)).unwrap();
    let mut other_luks = holding(&[(0, with_bytes(zero, 0, b"LUKS\xba\xbe\x00\x01"))]);
    assert_eq!(found(&mut other_luks), Found::Luks);
    for (why, mut swapped) in [
        ("blank", Mem::new(disk_sectors())),
        ("ext4", ext.clone()),
        ("another LUKS", other_luks),
    ] {
        let got = apply(&over, &mut running.disk.clone(), &mut swapped);
        assert_eq!(got, Err(Error::Target), "{why}");
        assert!(swapped.log.is_empty(), "{why}: wrote");
    }
    // Applied to the disk it was planned over, it installs.
    apply(&over, &mut running.disk.clone(), &mut luks).unwrap();
    installed(&luks.cut(), &over, &running);
}
