//! The installer's plan over in-memory disks: the installed disk reads back
//! through `diskmap::read` and kaifs mounts its `kai-data`; installs get their
//! own GUIDs; and a cut after any write leaves either the old install whole or
//! a disk with no valid GPT, until the backup header lands on a whole new one.

use std::collections::BTreeMap;

use diskmap::{Guid, Spec, Table};
use installer::{
    CLEARED, Disk, Error, FOLDERS, IoError, KINDS, NAMES, PARTITION_MIB, Plan, Step, Target,
    Window, apply, data_first, minimum_sectors, plan,
};
use kaifs::Fs;

const S: usize = 512;
type Sector = [u8; S];

/// A sparse disk: unwritten sectors, and sectors written as zeros, are absent.
#[derive(Clone, Default, PartialEq, Eq)]
struct Mem {
    sectors: u64,
    data: BTreeMap<u64, Sector>,
    /// Every write, as (first LBA, sectors), and `None` for a flush.
    log: Vec<Option<(u64, Vec<Sector>)>>,
    logging: bool,
}

impl Mem {
    fn new(sectors: u64) -> Self {
        Self {
            sectors,
            ..Self::default()
        }
    }

    fn put(&mut self, lba: u64, s: Sector) {
        match s == [0; S] {
            true => self.data.remove(&lba),
            false => self.data.insert(lba, s),
        };
    }

    fn get(&self, lba: u64) -> Sector {
        self.data.get(&lba).copied().unwrap_or([0; S])
    }

    fn writes(&self) -> impl Iterator<Item = &(u64, Vec<Sector>)> {
        self.log.iter().flatten()
    }
}

impl Disk for Mem {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), IoError> {
        let n = (out.len() / S) as u64;
        if lba + n > self.sectors {
            return Err(IoError);
        }
        for (i, chunk) in out.as_chunks_mut::<S>().0.iter_mut().enumerate() {
            chunk.copy_from_slice(&self.get(lba + i as u64));
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), IoError> {
        let n = (data.len() / S) as u64;
        if lba + n > self.sectors {
            return Err(IoError);
        }
        let sectors: Vec<Sector> = data.as_chunks::<S>().0.to_vec();
        for (i, s) in sectors.iter().enumerate() {
            self.put(lba + i as u64, *s);
        }
        if self.logging {
            self.log.push(Some((lba, sectors)));
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        if self.logging {
            self.log.push(None);
        }
        Ok(())
    }
}

impl diskmap::Disk for Mem {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        Disk::read(self, lba, out).map_err(|_| diskmap::Error::Read)
    }
}

/// A target with 64 MiB for `kai-data`.
fn disk_sectors() -> u64 {
    data_first() + 64 * 2048 + 34
}

/// Sectors of each running volume: small, so the tests stay quick.
const VOLUME: [u32; 3] = [4096, 2048, 3000];

/// A 512-byte-sector FAT boot sector for a volume of `total` sectors.
fn boot_sector(total: u32) -> Sector {
    let mut s = [0u8; S];
    s[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    s[11..13].copy_from_slice(&512u16.to_le_bytes());
    s[32..36].copy_from_slice(&total.to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn pattern(lba: u64, seed: u8) -> Sector {
    let mut s = [0u8; S];
    for (i, b) in s.iter_mut().enumerate() {
        *b = ((lba as u8).wrapping_mul(31) ^ (i as u8) ^ seed) | 1;
    }
    s
}

/// The running disk: a GPT of the ADR's layout, a FAT volume of
/// [`VOLUME`] sectors in the ESP, `kai-r` and `kai-a`, and bytes past each
/// volume, in `kai-b` and in `kai-data` that no install may copy.
fn running(seed: u8) -> (Mem, Table) {
    let mut disk = Mem::new(disk_sectors());
    let mut first = installer::FIRST_LBA;
    let mut specs = Vec::new();
    for (n, name) in NAMES.iter().enumerate() {
        let count = PARTITION_MIB.get(n).map_or(64 * 2048, |mib| mib * 2048);
        specs.push(Spec {
            kind: KINDS[n],
            guid: Guid(installer::unique("running", name)),
            first,
            last: first + count - 1,
            name,
        });
        first += count;
    }
    for (lba, bytes) in
        diskmap::layout(disk.sectors, Guid(installer::guid("running")), &specs).unwrap()
    {
        Disk::write(&mut disk, lba, &bytes).unwrap();
    }
    for (n, spec) in specs.iter().enumerate() {
        match VOLUME.get(n) {
            Some(&total) => {
                disk.put(spec.first, boot_sector(total));
                for lba in spec.first + 1..spec.first + u64::from(total) {
                    disk.put(lba, pattern(lba, seed + n as u8));
                }
                disk.put(spec.first + u64::from(total), [0xEE; S]);
            }
            None => disk.put(spec.first, [0xDD; S]),
        }
    }
    let table = diskmap::read(&mut disk).unwrap();
    (disk, table)
}

fn target(id: &str) -> Target<'_> {
    Target {
        sectors: disk_sectors(),
        sector_size: 512,
        id,
        now: 1_790_000_000,
    }
}

/// `plan` for `id`, applied to `disk` from a fresh running disk.
fn install(disk: &mut Mem, id: &str) -> Plan {
    let (mut source, table) = running(7);
    let plan = plan(target(id), &table).unwrap();
    apply(&plan, &mut source, disk).unwrap();
    plan
}

/// Checks that `disk` is a whole install of `plan` from [`running`].
fn installed(disk: &mut Mem, plan: &Plan) {
    let table = diskmap::read(disk).unwrap();
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
    let (source, from) = running(7);
    for n in 0..3 {
        let (at, to) = (from.partitions[n].first, plan.parts[n].first);
        for i in 0..u64::from(VOLUME[n]) {
            assert!(
                disk.get(to + i) == source.get(at + i),
                "{} sector {i}",
                NAMES[n]
            );
        }
        // Nothing past the volume is copied.
        assert!(disk.get(to + u64::from(VOLUME[n])) != [0xEE; S]);
    }
    let b = &plan.parts[3];
    assert!((b.first..b.first + CLEARED).all(|lba| disk.get(lba) == [0; S]));
    let data = &plan.parts[4];
    assert!((data.first..data.first + 8).all(|lba| disk.get(lba) == [0; S]));
    let window = Window {
        disk,
        first: data.first,
        blocks: data.sectors() / 8,
    };
    let mut fs = Fs::mount(window).expect("kai-data mounts");
    for (vol, path) in FOLDERS {
        fs.set_volume(vol).unwrap();
        fs.resolve(path).unwrap_or_else(|e| panic!("{path}: {e:?}"));
    }
}

#[test]
fn an_install_reads_back_and_its_data_mounts() {
    let mut disk = Mem::new(disk_sectors());
    let plan = install(&mut disk, "install-1");
    installed(&mut disk, &plan);
    assert!(!diskmap::read(&mut disk).unwrap().from_backup);
    // ADR 0008 §1: 2,368 MiB before kai-data, which starts on a MiB.
    assert_eq!(plan.parts[4].first, 2048 * (1 + 2368));
    assert_eq!(plan.parts[4].sectors() % 8, 0);
    let sizes: Vec<u64> = plan.parts[..4].iter().map(|p| p.sectors() / 2048).collect();
    assert_eq!(sizes, PARTITION_MIB);
}

#[test]
fn two_installs_share_no_guid_and_one_id_writes_the_same_disk() {
    let (mut a, mut again, mut b) = (
        Mem::new(disk_sectors()),
        Mem::new(disk_sectors()),
        Mem::new(disk_sectors()),
    );
    let (pa, _, pb) = (
        install(&mut a, "install-a"),
        install(&mut again, "install-a"),
        install(&mut b, "install-b"),
    );
    assert!(a.data == again.data, "one id wrote two disks");
    let guids: Vec<Guid> = [&pa, &pb]
        .iter()
        .flat_map(|p| p.parts.iter().map(|x| x.guid).chain([p.disk_guid]))
        .collect();
    for (i, g) in guids.iter().enumerate() {
        assert!(!guids[i + 1..].contains(g), "a GUID repeats");
        assert!(!diskmap::TYPES.contains(g));
    }
    // The volume id of kai-data differs too.
    let uuid = |p: &Plan| {
        p.steps.iter().find_map(|s| match s {
            Step::Format { uuid, .. } => Some(*uuid),
            _ => None,
        })
    };
    assert_ne!(uuid(&pa), uuid(&pb));
    // xtask's release image derives its GUIDs the same way.
    assert_eq!(pa.disk_guid, Guid(installer::guid("install-a disk")));
}

/// Over an old install, cut after each write: the old install, whole, or no
/// valid GPT, until the backup header lands on a whole new install.
#[test]
fn a_cut_after_any_write_never_leaves_a_half_install_that_boots() {
    let mut old = Mem::new(disk_sectors());
    let old_plan = install(&mut old, "old");
    // What an old install's use leaves: a slot in kai-b, a model hint.
    old.put(old_plan.parts[3].first, boot_sector(2048));
    old.put(old_plan.parts[4].first, [0x4B; S]);
    let old_table = diskmap::read(&mut old).unwrap();

    let mut logged = old.clone();
    logged.logging = true;
    let plan = install(&mut logged, "new");
    let last = disk_sectors() - 1;
    let writes: Vec<_> = logged.writes().cloned().collect();
    let backup_at = writes.iter().position(|(lba, _)| *lba == last).unwrap();
    assert_eq!(
        backup_at,
        writes.len() - 2,
        "the backup header is next to last"
    );
    assert_eq!(writes.last().unwrap().0, 1, "the primary header is last");
    // The first flush ends the zeroing of both old tables.
    let first_flush = logged.log.iter().position(Option::is_none).unwrap();

    let mut disk = old.clone();
    for (k, (lba, sectors)) in writes.iter().enumerate() {
        for (i, s) in sectors.iter().enumerate() {
            disk.put(lba + i as u64, *s);
        }
        let read = diskmap::read(&mut disk);
        match k {
            k if k < first_flush => {
                if let Ok(t) = read {
                    assert_eq!(t.partitions, old_table.partitions, "after write {k}");
                }
            }
            k if k < backup_at => assert!(read.is_err(), "a table after write {k}"),
            k if k == backup_at => {
                assert!(read.unwrap().from_backup);
                installed(&mut disk.clone(), &plan);
            }
            _ => assert!(!read.unwrap().from_backup),
        }
        // Before the backup header, no write after the first flush touches
        // either header's sector, in whatever order the flush's writes land.
        if (first_flush..backup_at).contains(&k) {
            let end = lba + sectors.len() as u64;
            assert!(!(*lba..end).contains(&1) && !(*lba..end).contains(&last));
        }
    }
    installed(&mut disk, &plan);
}

#[test]
fn a_disk_too_small_or_not_512_byte_sectors_is_refused() {
    let (_, table) = running(7);
    let at = |sectors: u64, sector_size: u32| {
        plan(
            Target {
                sectors,
                sector_size,
                id: "x",
                now: 0,
            },
            &table,
        )
    };
    let need = minimum_sectors();
    assert_eq!(at(need - 1, 512), Err(Error::TooSmall { need }));
    assert_eq!(at(disk_sectors(), 4096), Err(Error::SectorSize(4096)));
    // 2,368 MiB and kai-data's minimum: it installs, and kai-data mounts.
    let plan = at(need, 512).unwrap();
    assert_eq!(plan.parts[4].sectors(), installer::DATA_MIN_BLOCKS * 8);
    let (mut source, _) = running(7);
    let mut disk = Mem::new(need);
    apply(&plan, &mut source, &mut disk).unwrap();
    let data = &plan.parts[4];
    Fs::mount(Window {
        disk: &mut disk,
        first: data.first,
        blocks: data.sectors() / 8,
    })
    .expect("the smallest kai-data mounts");
    // A target not the plan's size is refused before any write.
    let mut other = Mem::new(need + 8);
    assert_eq!(apply(&plan, &mut source, &mut other), Err(Error::Target));
    assert!(other.data.is_empty());
}

#[test]
fn bad_ids_are_refused() {
    let (_, table) = running(7);
    for id in ["", "has space", &"x".repeat(65), "tab\t"] {
        assert_eq!(plan(target(id), &table), Err(Error::BadId), "{id:?}");
    }
}

/// A running disk that cannot be copied is refused, and nothing is written.
#[test]
fn a_running_disk_that_cannot_be_copied_writes_nothing() {
    let (_, table) = running(7);
    let mut missing = table.clone();
    missing.partitions.retain(|p| p.kind != diskmap::KAI_A_TYPE);
    assert!(matches!(plan(target("x"), &missing), Err(Error::Source(_))));
    let mut larger = table.clone();
    larger.partitions[1].last += 1;
    assert!(matches!(plan(target("x"), &larger), Err(Error::Source(_))));

    let plan = plan(target("x"), &table).unwrap();
    let r = table.partitions[1].first;
    for (why, boot) in [
        ("no FAT volume", [0u8; S]),
        ("a volume past its partition", boot_sector(256 * 2048 + 1)),
        ("an empty volume", boot_sector(0)),
    ] {
        let (mut source, _) = running(7);
        source.put(r, boot);
        let mut disk = Mem::new(disk_sectors());
        disk.logging = true;
        assert!(
            matches!(apply(&plan, &mut source, &mut disk), Err(Error::Source(_))),
            "{why}"
        );
        assert!(disk.log.is_empty(), "{why}: wrote");
    }
}
