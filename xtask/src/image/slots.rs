//! The slot disk of ADR 0008 section 1: the ESP with Limine and the constant
//! `limine.conf`, then `kai-r`, `kai-a` and `kai-b`, each a FAT32 volume with
//! the kernel, the bundle and the slot's `kai-slot 1` (`boot/slot`). The config
//! is never rewritten: the kernel picks the next boot with Limine's one-shot
//! variable (`kernel/src/slots.rs`), and no one-shot boots R.
//!
//! The partitions are the ADR's fixed sizes ([`PARTITION_MIB`]), not this
//! build's. An install's slots never grow, since `kai-data` follows `kai-b`,
//! so a later release's larger slot image must still fit them (bug-hunt-4
//! H4-16). Each slot's volume is its files' size
//! ([`fat32::fitting_sectors`]) at the start of its partition, as `updated`
//! writes a slot image; the rest of the partition is zeros.
//!
//! The release disk adds `kai-data` as partition 5 ([`build_release`]), which
//! `fs` finds by its type as it finds a disk of its own; the boot cases attach
//! `kai-data` as a disk of its own.

use super::fat32::{self, Node};
use super::{MIB, SECTOR, gpt};
use std::fs;
use std::path::Path;

/// The slots' GPT names, which are also their type GUIDs' labels.
pub const SLOTS: [&str; 3] = ["kai-r", "kai-a", "kai-b"];
/// ADR 0008 section 1's partition sizes in MiB, in disk order: the ESP,
/// `kai-r`, `kai-a` and `kai-b`. 2,368 MiB before `kai-data`.
pub const PARTITION_MIB: [u64; 4] = installer::PARTITION_MIB;
/// Volume ids, one per partition, fixed so the image is deterministic.
const VOLUME_IDS: [u32; 4] = [0x4B41_4931, 0x4B41_4952, 0x4B41_4941, 0x4B41_4942];
/// The slot disk's default image id, which seeds the disk's and partitions'
/// GUIDs and `kai-data`'s volume id ([`gpt::unique`]). Every boot case and CI
/// keep it, so their disks are byte-identical; `release-image --image-id`
/// (later the installer) gives one image its own.
pub const DISK_NAME: &str = "kernel-ai-slots";

/// ADR 0008 section 3's `limine.conf`. `cmdline` is the dev and test words; an
/// installed system's is empty. `slot=` names the slot, since Limine 12
/// reports no partition for a `boot(N):` file (decisions.md, S08-1).
pub fn limine_conf(cmdline: &str) -> String {
    let mut conf = String::from("timeout: 0\ndefault_entry: 1\n");
    for (entry, partition, words) in [
        ("recovery", 2, "slot=r mode=recovery"),
        ("kai-a", 3, "slot=a"),
        ("kai-b", 4, "slot=b"),
    ] {
        let line = [cmdline.trim(), words].join(" ");
        conf.push_str(&format!(
            "\n/{entry}\n    protocol: limine\n    path: boot({partition}):/boot/kernel\n    \
             cmdline: {}\n    module_path: boot({partition}):/boot/{}\n",
            line.trim(),
            crate::esp::BUNDLE
        ));
    }
    conf
}

/// The `kai-slot 1` record of this build (ADR 0008 section 2), for `boot/slot`
/// and the bundle's `kai.slot`. Its version is the workspace's
/// (`[workspace.package]`), which every crate inherits.
pub fn slot_record(root: &Path, release_seq: u32) -> Result<String, String> {
    let version = crate::notices::workspace_version(root)?;
    Ok(format!(
        "kai-slot 1\nrelease_seq {release_seq}\nepoch 1\nversion {version}\ncommit {}\nchannel dev\n",
        crate::esp::commit_id(root)
    ))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn file(name: &str, bytes: Vec<u8>) -> Node {
    Node::File {
        name: name.into(),
        bytes,
    }
}

fn dir(name: &str, children: Vec<Node>) -> Node {
    Node::Dir {
        name: name.into(),
        children,
    }
}

/// One slot's files: the kernel and bundle of the ESP's `boot`, and `record`.
fn slot_tree(boot: &Path, record: &str) -> Result<Vec<Node>, String> {
    Ok(vec![dir(
        "boot",
        vec![
            file("kernel", read(&boot.join("kernel"))?),
            file(crate::esp::BUNDLE, read(&boot.join(crate::esp::BUNDLE))?),
            file("slot", record.as_bytes().to_vec()),
        ],
    )])
}

/// A slot image on its own (ADR 0008 §2): the volume `updated` writes raw into
/// a slot partition, as an update carries it. It is its files' size, with no
/// room to spare, so a download is the image's size; it fits any install's
/// `kai-a` and `kai-b` while it is under their [`PARTITION_MIB`].
pub fn slot_image(esp: &Path, record: &str) -> Result<Vec<u8>, String> {
    let tree = slot_tree(&esp.join("boot"), record)?;
    let sectors = fat32::fitting_sectors(&tree)?;
    fat32::volume(sectors, 0, VOLUME_IDS[2], &tree)
}

/// What a slot disk does with a volume larger than its partition's
/// [`PARTITION_MIB`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Room {
    /// The release disk refuses it: an install keeps the partitions it was
    /// written with.
    Fixed,
    /// A boot case's disk grows that partition to the volume: a test build is
    /// larger than a release (219 MiB of slot files on 2026-11-25, the
    /// release's 113 MiB) and is never installed.
    Grow,
}

/// The sectors of partition `name`, ADR 0008's `mib` MiB, for a volume of
/// `need` sectors: by `room`, refused or grown when the volume is larger.
fn partition_sectors(name: &str, mib: u64, need: u32, room: Room) -> Result<u64, String> {
    let sectors = mib * MIB / u64::from(SECTOR);
    let need = u64::from(need);
    match room {
        _ if need <= sectors => Ok(sectors),
        Room::Grow => Ok(need.next_multiple_of(2048)),
        Room::Fixed => Err(format!(
            "{name}'s files need a {} MiB volume, more than its {mib} MiB partition \
             (ADR 0008 section 1). A release disk keeps the ADR's sizes, since an \
             installed disk's partitions never grow: revisit the slot sizes before \
             this ships (ADR 0008, S08-2)",
            need.div_ceil(2048)
        )),
    }
}

/// A slot disk laid out: its size in sectors, its GPT's sectors, each
/// partition's volume by its first LBA, each partition as (first LBA,
/// sectors) in disk order, and `kai-data`'s place if it has one.
pub(super) struct Plan {
    sectors: u64,
    gpt: Vec<(u64, Vec<u8>)>,
    volumes: Vec<(u64, Vec<u8>)>,
    partitions: Vec<(u64, u64)>,
    pub(super) data: Option<(u64, u64)>,
}

impl Plan {
    /// The disk's size in bytes.
    pub(super) fn bytes(&self) -> u64 {
        self.sectors * u64::from(SECTOR)
    }

    /// Slot B's partition: the last before `kai-data`, in [`SLOTS`]' order.
    fn slot_b(&self) -> Result<(u64, u64), String> {
        self.partitions
            .last()
            .copied()
            .ok_or_else(|| "the slot disk has no slot B".into())
    }
}

/// The layout for the directory ESP `esp`, whose kernel and bundle every slot
/// gets, with `record` as each slot's `boot/slot`, and after the slots a
/// `kai-data` partition of `data_mib` MiB if asked for. The partitions are
/// [`PARTITION_MIB`]; `room` says what a larger volume does.
pub(super) fn plan(
    esp: &Path,
    cmdline: &str,
    record: &str,
    data_mib: Option<u64>,
    room: Room,
    image_id: &str,
    sector: usize,
) -> Result<Plan, String> {
    let boot = esp.join("boot");
    let mut esp_files = vec![
        dir(
            "EFI",
            vec![dir(
                "BOOT",
                vec![file(
                    "BOOTX64.EFI",
                    read(&esp.join("EFI/BOOT/BOOTX64.EFI"))?,
                )],
            )],
        ),
        dir(
            "boot",
            vec![dir(
                "limine",
                vec![file("limine.conf", limine_conf(cmdline).into_bytes())],
            )],
        ),
    ];
    let notices = esp.join(crate::notices::FILE);
    if notices.exists() {
        esp_files.push(file(crate::notices::FILE, read(&notices)?));
    }
    let mut trees = vec![esp_files];
    for _ in SLOTS {
        trees.push(slot_tree(&boot, record)?);
    }
    let mut volumes = Vec::new();
    let mut partitions = Vec::new();
    let mut specs = Vec::new();
    let mut first = gpt::FIRST_LBA;
    for (n, ((tree, id), mib)) in trees.iter().zip(VOLUME_IDS).zip(PARTITION_MIB).enumerate() {
        let (kind, name) = match n {
            0 => (diskmap::ESP_TYPE, "EFI system partition"),
            _ => (
                diskmap::Guid(gpt::guid(&format!("{} partition type", SLOTS[n - 1]))),
                SLOTS[n - 1],
            ),
        };
        let need = fat32::fitting_sectors(tree).map_err(|e| format!("{name}: {e}"))?;
        let count = partition_sectors(name, mib, need, room)?;
        if count > mib * MIB / u64::from(SECTOR) {
            println!(
                "xtask: {name} is {} MiB for this test build, past ADR 0008's {mib} MiB",
                count / 2048
            );
        }
        // The ESP's volume fills its partition, which the firmware reads and P8's
        // installer writes again; a slot's is its files' size, as an update's is.
        let sectors = match n {
            0 => count,
            _ => u64::from(need),
        };
        let sectors = u32::try_from(sectors).map_err(|_| "a partition would be too large")?;
        let hidden = u32::try_from(first).map_err(|_| "a partition starts too far in")?;
        volumes.push((first, fat32::volume(sectors, hidden, id, tree)?));
        specs.push((kind, name, first, first + count - 1));
        partitions.push((first, count));
        first = (first + count).next_multiple_of(2048);
    }
    // Partition 5 (ADR 0004): starts on a MiB, so on a kaifs block too.
    let data = data_mib.map(|mib| (first, mib * MIB / u64::from(SECTOR)));
    if let Some((at, count)) = data {
        specs.push((diskmap::KAI_DATA_TYPE, DATA_NAME, at, at + count - 1));
        first = at + count;
    }
    let parts: Vec<diskmap::Spec<'_>> = specs
        .iter()
        .map(|&(kind, name, first, last)| diskmap::Spec {
            kind,
            guid: diskmap::Guid(gpt::unique(image_id, &format!("partition {name}"))),
            first,
            last,
            name,
        })
        .collect();
    let sectors = gpt::disk_sectors(first - gpt::FIRST_LBA);
    let disk_guid = diskmap::Guid(gpt::unique(image_id, "disk"));
    let gpt = gpt::layout_on(sector, sectors, disk_guid, &parts)
        .map_err(|e| format!("the slot disk: {e}"))?;
    Ok(Plan {
        sectors,
        gpt,
        volumes,
        partitions,
        data,
    })
}

/// Writes `plan`'s disk to `out` as a new file: the GPT and each volume where
/// the layout puts it, a piece at a time, and zeros everywhere else, left
/// sparse where the system allows. The disk is gigabytes, most of them the
/// slots' free room, and is never held in memory. With `keep`, the range
/// `(first, sectors)` is that disk's, as it is, in place of a new volume.
pub(super) fn write(
    plan: &Plan,
    out: &Path,
    keep: Option<(&Path, (u64, u64))>,
) -> Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};
    let shown = out.display();
    let fail = |e: std::io::Error| format!("cannot write {shown}: {e}");
    let mut file = fs::File::create(out).map_err(fail)?;
    file.set_len(plan.bytes()).map_err(fail)?;
    let volumes = plan
        .volumes
        .iter()
        .filter(|(lba, _)| keep.is_none_or(|(_, (first, _))| *lba != first));
    for (lba, bytes) in plan.gpt.iter().chain(volumes) {
        file.seek(SeekFrom::Start(lba * u64::from(SECTOR)))
            .and_then(|_| file.write_all(bytes))
            .map_err(fail)?;
    }
    if let Some((old, (first, sectors))) = keep {
        let sector = u64::from(SECTOR);
        super::slot_data::copy_range(old, &mut file, first * sector, sectors * sector)?;
    }
    file.sync_all().map_err(fail)
}

/// [`build`], but slot B keeps the bytes it holds in the disk already at `out`
/// when that disk has the same layout. A machine that reboots between two runs
/// of `updated` has the slot it was writing as it left it, so a resumed download
/// finds the chunks the first run wrote; `build` makes every boot's slots anew.
pub fn build_keeping_b(esp: &Path, out: &Path, cmdline: &str, record: &str) -> Result<u64, String> {
    let plan = plan(esp, cmdline, record, None, Room::Grow, DISK_NAME, 512)?;
    let same = fs::metadata(out).is_ok_and(|old| old.len() == plan.bytes());
    if !same {
        write(&plan, out, None)?;
        return Ok(plan.bytes());
    }
    // The new disk beside the old, whose B it copies, then in its place.
    let next = out.with_extension("next");
    write(&plan, &next, Some((out, plan.slot_b()?)))?;
    fs::rename(&next, out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(plan.bytes())
}

/// Writes the slot disk to `out`, for a disk of `sector`-byte sectors; its size.
pub fn build(
    esp: &Path,
    out: &Path,
    cmdline: &str,
    record: &str,
    sector: usize,
) -> Result<u64, String> {
    let plan = plan(esp, cmdline, record, None, Room::Grow, DISK_NAME, sector)?;
    write(&plan, out, None)?;
    Ok(plan.bytes())
}

/// `kai-data`'s GPT name, which is also its type GUID's label.
const DATA_NAME: &str = "kai-data";
/// The release image's `kai-data`, in MiB: room for Qwen3-4B (2.5 GB) and a
/// second model beside it, with the home folder.
pub const RELEASE_DATA_MIB: u64 = 8192;

/// A release image's command line: no dev or test words. Never `online=test`,
/// which opens QEMU's test subnet to names' answers (audit K #17).
pub const RELEASE_CMDLINE: &str = "";

/// Whether `cmdline` may go on a release image: no word that only the boot
/// cases may use.
pub fn release_cmdline_ok(cmdline: &str) -> bool {
    !cmdline
        .split_whitespace()
        .any(|word| word.starts_with("online=") || word.starts_with("test="))
}

/// A slot warns when its files take more than this many percent of its
/// partition: the partitions never grow on an install (ADR 0008 S08-2).
pub const FILL_WARN_PERCENT: u64 = 80;

/// One line for slot `name` whose volume takes `used` bytes of a `mib` MiB
/// partition: its size in MiB, the share of the partition to a tenth of a
/// percent, and a warning past [`FILL_WARN_PERCENT`] (exactly that is none).
/// The line starts `xtask: slot `, which the release workflow's summary reads.
pub fn fill_line(name: &str, used: u64, mib: u64) -> String {
    let capacity = mib * MIB;
    let tenths_mib = used * 10 / MIB;
    let tenths_percent = used * 1000 / capacity;
    let mut line = format!(
        "xtask: slot {name} {}.{} MiB of {mib} MiB ({}.{}%)",
        tenths_mib / 10,
        tenths_mib % 10,
        tenths_percent / 10,
        tenths_percent % 10
    );
    if used * 100 > capacity * FILL_WARN_PERCENT {
        line.push_str(&format!(" WARNING: more than {FILL_WARN_PERCENT}% full"));
    }
    line
}

/// [`fill_line`] for each slot of `plan`, in disk order.
fn fill_lines(plan: &Plan) -> Vec<String> {
    let volumes = plan.volumes.iter().skip(1);
    let sizes = PARTITION_MIB.iter().skip(1);
    SLOTS
        .iter()
        .zip(volumes.zip(sizes))
        .map(|(name, ((_, volume), &mib))| fill_line(name, volume.len() as u64, mib))
        .collect()
}

/// Writes the release disk to `out`: the slot disk with an empty command line
/// and exactly [`PARTITION_MIB`] (a build whose files do not fit is refused),
/// then `kai-data` as partition 5, formatted with a fixed volume id, so every
/// build of a commit writes the same bytes. `updated` never writes it: it
/// writes only `kai-a` and `kai-b`. The disk is written a piece at a time and
/// left sparse where the system allows, never held in memory. `image_id`
/// seeds its disk, partition and `kai-data` volume GUIDs ([`DISK_NAME`] for
/// CI's). Its size.
pub fn build_release(
    esp: &Path,
    out: &Path,
    record: &str,
    data_mib: u64,
    image_id: &str,
) -> Result<u64, String> {
    if !release_cmdline_ok(RELEASE_CMDLINE) {
        return Err("a release image may not carry a test word".into());
    }
    let plan = plan(
        esp,
        RELEASE_CMDLINE,
        record,
        Some(data_mib),
        Room::Fixed,
        image_id,
        512,
    )?;
    for line in fill_lines(&plan) {
        println!("{line}");
    }
    write(&plan, out, None)?;
    let (first, sectors) = plan.data.ok_or("the release disk has no kai-data")?;
    let uuid = gpt::unique(image_id, "kai-data volume");
    crate::fs_tool::format_partition(out, first, sectors, uuid)?;
    Ok(plan.bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;

    #[test]
    fn a_release_image_never_carries_online_test() {
        assert!(release_cmdline_ok(RELEASE_CMDLINE));
        let conf = limine_conf(RELEASE_CMDLINE);
        assert!(!conf.contains("online="), "{conf}");
        assert!(!release_cmdline_ok("online=test"));
        assert!(!release_cmdline_ok("accel=kvm online=test"));
        assert!(!release_cmdline_ok("test=update"));
    }

    #[test]
    fn the_config_names_every_slot_by_partition_and_defaults_to_r() {
        let conf = limine_conf("test=slot");
        assert!(conf.starts_with("timeout: 0\ndefault_entry: 1\n\n/recovery\n"));
        for (entry, n) in [("recovery", 2), ("kai-a", 3), ("kai-b", 4)] {
            assert!(conf.contains(&format!("/{entry}\n")));
            assert!(conf.contains(&format!("path: boot({n}):/boot/kernel")));
            assert!(conf.contains(&format!("module_path: boot({n}):/boot/kai.bundle")));
        }
        assert!(conf.contains("cmdline: test=slot slot=r mode=recovery\n"));
        assert!(conf.contains("cmdline: test=slot slot=b\n"));
        assert!(!conf.contains("remember_last_entry"));
        // An installed system's command line names only the slot.
        assert!(limine_conf("").contains("cmdline: slot=a\n"));
    }

    /// The record's version is the workspace's, not `0.0.0` (bug-hunt-4
    /// H4-17): `kernel/Cargo.toml` only inherits it.
    #[test]
    fn the_slot_record_reads_back_with_bootslot_and_the_workspace_version() {
        let root = crate::workspace_root();
        let record = slot_record(&root, 5).unwrap();
        let file = bootslot::slotfile::parse(&record).unwrap();
        assert_eq!((file.release_seq, file.epoch, file.channel), (5, 1, "dev"));
        // xtask's own version is the workspace's too.
        assert_eq!(file.version, env!("CARGO_PKG_VERSION"));
        assert_ne!(file.version, "0.0.0");
        assert_eq!(
            crate::notices::workspace_version(&root).unwrap(),
            file.version
        );
    }

    const RECORD: &str = "kai-slot 1\nrelease_seq 1\nepoch 1\n";

    /// A directory ESP with stand-in files, under the system's temp directory.
    fn fake_esp(name: &str) -> PathBuf {
        let esp = std::env::temp_dir().join(format!("kai-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&esp);
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(esp.join("boot")).unwrap();
        fs::write(esp.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        fs::write(esp.join("boot/kernel"), b"kernel").unwrap();
        fs::write(esp.join("boot/kai.bundle"), b"bundle").unwrap();
        esp
    }

    /// A disk image file, read through `diskmap` a few sectors at a time: the
    /// disks are gigabytes, and sparse.
    struct FileDisk(fs::File);
    impl diskmap::Disk for FileDisk {
        fn sectors(&self) -> u64 {
            self.0.metadata().map_or(0, |m| m.len() / 512)
        }
        fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
            self.0
                .seek(SeekFrom::Start(lba * 512))
                .and_then(|_| self.0.read_exact(out))
                .map_err(|_| diskmap::Error::Read)
        }
    }

    fn partitions(disk: &Path) -> Vec<diskmap::Partition> {
        let file = fs::File::open(disk).unwrap();
        diskmap::read(&mut FileDisk(file)).unwrap().partitions
    }

    fn read_at(disk: &Path, at: u64, len: usize) -> Vec<u8> {
        let mut file = fs::File::open(disk).unwrap();
        let mut out = vec![0; len];
        file.seek(SeekFrom::Start(at)).unwrap();
        file.read_exact(&mut out).unwrap();
        out
    }

    fn write_at(disk: &Path, at: u64, bytes: &[u8]) {
        let mut file = fs::OpenOptions::new().write(true).open(disk).unwrap();
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(bytes).unwrap();
    }

    /// Whether the files `a` and `b` hold the same bytes from `from` to `to`.
    fn same(a: &Path, b: &Path, from: u64, to: u64) -> bool {
        let mut at = from;
        while at < to {
            let n = (to - at).min(16 * MIB) as usize;
            if read_at(a, at, n) != read_at(b, at, n) {
                return false;
            }
            at += n as u64;
        }
        true
    }

    /// The total sectors in the FAT32 boot sector at `lba`.
    fn volume_sectors(disk: &Path, lba: u64) -> u32 {
        let field = read_at(disk, lba * 512 + 32, 4);
        u32::from_le_bytes(field.try_into().unwrap())
    }

    #[test]
    fn the_release_disk_adds_a_kai_data_that_checks_and_builds_the_same_bytes_twice() {
        let esp = fake_esp("release-disk");
        let (one, two) = (esp.join("one.img"), esp.join("two.img"));
        let len = build_release(&esp, &one, RECORD, 32, DISK_NAME).unwrap();
        build_release(&esp, &two, RECORD, 32, DISK_NAME).unwrap();
        assert_eq!(fs::metadata(&one).unwrap().len(), len);
        assert!(same(&one, &two, 0, len), "two builds differ");
        // Its first four partitions hold the slot disk's own bytes.
        let slots = esp.join("slots.img");
        build(&esp, &slots, "", RECORD, 512).unwrap();
        let parts = partitions(&one);
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "EFI system partition",
                "kai-r",
                "kai-a",
                "kai-b",
                "kai-data"
            ]
        );
        assert_eq!(parts[4].kind, diskmap::KAI_DATA_TYPE);
        assert_eq!(parts[4].sectors(), 32 * 2048);
        assert!(same(&one, &slots, 2048 * 512, (parts[3].last + 1) * 512));
        // kaifs on it mounts and checks, with its first folders.
        let path = one.display().to_string();
        crate::fs_tool::main(&["check".into(), path.clone()]).unwrap();
        crate::fs_tool::main(&["ls".into(), path, "/home/user".into()]).unwrap();
        fs::remove_dir_all(&esp).unwrap();
    }

    /// G35 (ADR 0008 §1): an image id gives the disk and every partition GUIDs
    /// of their own, the same id the same bytes, and no type GUID, partition
    /// or slot byte moves.
    #[test]
    fn an_image_id_gives_its_own_guids_and_keeps_types_and_slots() {
        let esp = fake_esp("image-id");
        let (a, again, b) = (esp.join("a.img"), esp.join("a2.img"), esp.join("b.img"));
        let len = build_release(&esp, &a, RECORD, 32, "install-a").unwrap();
        build_release(&esp, &again, RECORD, 32, "install-a").unwrap();
        build_release(&esp, &b, RECORD, 32, "install-b").unwrap();
        assert!(same(&a, &again, 0, len), "one id built two different disks");
        let table =
            |disk: &Path| diskmap::read(&mut FileDisk(fs::File::open(disk).unwrap())).unwrap();
        let (ta, tb) = (table(&a), table(&b));
        assert_ne!(ta.disk_guid, tb.disk_guid);
        assert_eq!(ta.partitions.len(), tb.partitions.len());
        for (pa, pb) in ta.partitions.iter().zip(&tb.partitions) {
            assert_ne!(pa.guid, pb.guid, "{}", pa.name);
            assert_eq!((pa.kind, pa.first, pa.last), (pb.kind, pb.first, pb.last));
            assert_eq!(pa.name, pb.name);
            assert!(!diskmap::TYPES.contains(&pa.guid));
        }
        let guids: Vec<diskmap::Guid> = [&ta, &tb]
            .iter()
            .flat_map(|t| t.partitions.iter().map(|p| p.guid).chain([t.disk_guid]))
            .collect();
        for (i, g) in guids.iter().enumerate() {
            assert!(
                guids[i + 1..].iter().all(|other| other != g),
                "a GUID repeats"
            );
        }
        // The ESP and the slots hold the same bytes under either id.
        let parts = &ta.partitions;
        assert!(same(
            &a,
            &b,
            parts[0].first * 512,
            (parts[3].last + 1) * 512
        ));
        // CI's default id is today's fixed seed: its GUIDs come from the same
        // labels as before per-image ids.
        assert_eq!(
            gpt::unique(DISK_NAME, "disk"),
            gpt::guid("kernel-ai-slots disk")
        );
        assert_eq!(
            gpt::unique(DISK_NAME, "partition kai-a"),
            gpt::guid("kernel-ai-slots partition kai-a")
        );
        crate::fs_tool::main(&["check".into(), b.display().to_string()]).unwrap();
        fs::remove_dir_all(&esp).unwrap();
    }

    /// Bug-hunt-4 H4-16: the release disk's partitions are ADR 0008's whatever
    /// this build holds, so the slot image of a later, larger release still fits
    /// an install's `kai-a` and `kai-b`. With slots sized to this build, a
    /// kernel 1 MiB larger was refused.
    #[test]
    fn a_later_larger_slot_image_fits_the_release_disks_slots() {
        let mib = MIB as usize;
        let now = fake_esp("release-now");
        fs::write(now.join("boot/kernel"), vec![0x5A; 40 * mib]).unwrap();
        let later = fake_esp("release-later");
        fs::write(later.join("boot/kernel"), vec![0x5A; 41 * mib]).unwrap();
        let out = now.join("release.img");
        let len = build_release(&now, &out, RECORD, 32, DISK_NAME).unwrap();
        let parts = partitions(&out);
        let sizes: Vec<u64> = parts.iter().map(|p| p.sectors() / 2048).collect();
        assert_eq!(sizes, [64, 256, 1024, 1024, 32]);
        // 2,368 MiB after the first, then kai-data, then the backup GPT's MiB.
        assert_eq!(parts[4].first, 2048 * (1 + 2368));
        assert_eq!(len, (1 + 2368 + 32 + 1) * MIB);
        let image = slot_image(&later, RECORD).unwrap();
        let own = slot_image(&now, RECORD).unwrap();
        assert!(image.len() > own.len(), "the later image is the larger");
        for slot in &parts[2..4] {
            assert!(image.len() as u64 <= slot.sectors() * 512, "{}", slot.name);
        }
        // Each slot's volume is this build's own image's size.
        for slot in &parts[1..4] {
            let sectors = u64::from(volume_sectors(&out, slot.first));
            assert_eq!(sectors * 512, own.len() as u64, "{}", slot.name);
        }
        fs::remove_dir_all(&now).unwrap();
        fs::remove_dir_all(&later).unwrap();
    }

    #[test]
    fn a_release_whose_files_outgrow_a_partition_is_refused_and_a_test_disk_grows() {
        let mib = |n: u64| n * 2048;
        assert_eq!(
            partition_sectors("kai-r", 256, 200 * 2048, Room::Fixed),
            Ok(mib(256))
        );
        let refused = partition_sectors("kai-r", 256, 300 * 2048, Room::Fixed).unwrap_err();
        for part in ["kai-r", "300 MiB", "256 MiB"] {
            assert!(refused.contains(part), "{refused}");
        }
        assert_eq!(
            partition_sectors("kai-r", 256, 300 * 2048, Room::Grow),
            Ok(mib(300))
        );
        assert_eq!(
            partition_sectors("kai-a", 1024, 1024 * 2048, Room::Fixed),
            Ok(mib(1024))
        );
    }

    #[test]
    fn slot_b_keeps_what_a_run_of_updated_wrote_into_it_and_nothing_else_does() {
        let esp = fake_esp("keep-b");
        let out = esp.join("slots.img");
        let len = build_keeping_b(&esp, &out, "test=slot", RECORD).unwrap();
        let parts = partitions(&out);
        let at = |p: usize| parts[p].first * 512;
        // Past the end of B's volume, as a larger update's chunks are.
        let past = at(3) + 512 * MIB;
        let fresh_b = read_at(&out, at(3) + 4096, 8);
        let fresh_a = read_at(&out, at(2) + 4096, 1);
        let esp_bytes = read_at(&out, at(0), 64 * MIB as usize);
        // The guest wrote chunks into B and a byte into A.
        write_at(&out, at(3) + 4096, b"chunk-b!");
        write_at(&out, past, b"chunk-2!");
        write_at(&out, at(2) + 4096, &[0xAA]);
        // The next boot's disk is rebuilt, B as the guest left it.
        build_keeping_b(&esp, &out, "test=update", RECORD).unwrap();
        assert_eq!(fs::metadata(&out).unwrap().len(), len);
        assert!(!out.with_extension("next").exists());
        assert_eq!(read_at(&out, at(3) + 4096, 8), b"chunk-b!");
        assert_eq!(read_at(&out, past, 8), b"chunk-2!");
        assert_eq!(read_at(&out, at(2) + 4096, 1), fresh_a, "A is made anew");
        // The command line is the new boot's, in the ESP.
        assert!(read_at(&out, at(0), 64 * MIB as usize) != esp_bytes);
        // `build` makes every slot anew.
        build(&esp, &out, "test=update", RECORD, 512).unwrap();
        assert_eq!(read_at(&out, at(3) + 4096, 8), fresh_b);
        assert_eq!(read_at(&out, past, 8), [0u8; 8]);
        // A disk of another size is not carried over.
        fs::write(&out, b"short").unwrap();
        build_keeping_b(&esp, &out, "test=update", RECORD).unwrap();
        assert_eq!(fs::metadata(&out).unwrap().len(), len);
        assert_eq!(read_at(&out, at(3) + 4096, 8), fresh_b);
        fs::remove_dir_all(&esp).unwrap();
    }

    #[test]
    fn a_slot_warns_past_eighty_percent_and_not_at_it() {
        // 100 MiB, so 80% is a whole number of bytes.
        let at = 80 * MIB;
        assert_eq!(
            fill_line("kai-a", at, 100),
            "xtask: slot kai-a 80.0 MiB of 100 MiB (80.0%)"
        );
        let over = fill_line("kai-a", at + 1, 100);
        assert!(over.ends_with("WARNING: more than 80% full"), "{over}");
        assert!(over.contains("(80.0%)"), "{over}");
        // A full partition, and an empty one.
        assert!(fill_line("kai-r", 256 * MIB, 256).contains("(100.0%) WARNING"));
        assert_eq!(
            fill_line("kai-b", 0, 1024),
            "xtask: slot kai-b 0.0 MiB of 1024 MiB (0.0%)"
        );
        // The release image's 113 MiB is far under it in kai-a's 1,024.
        assert_eq!(
            fill_line("kai-a", 113 * MIB, 1024),
            "xtask: slot kai-a 113.0 MiB of 1024 MiB (11.0%)"
        );
    }

    #[test]
    fn the_release_disk_reports_each_slots_fill() {
        let esp = fake_esp("fill");
        let record = "kai-slot 1\nrelease_seq 1\nepoch 1\nversion 0.1.0\ncommit x\nchannel dev\n";
        let plan = plan(&esp, "", record, Some(16), Room::Fixed, DISK_NAME, 512).unwrap();
        let lines = fill_lines(&plan);
        assert_eq!(lines.len(), 3);
        for (line, name) in lines.iter().zip(SLOTS) {
            assert!(line.starts_with(&format!("xtask: slot {name} ")), "{line}");
            assert!(!line.contains("WARNING"), "{line}");
        }
        assert!(lines[0].contains("of 256 MiB") && lines[1].contains("of 1024 MiB"));
    }

    #[test]
    fn the_disk_has_four_partitions_in_order_with_the_slot_types_and_the_adrs_sizes() {
        let esp = fake_esp("slots-test");
        let out = esp.join("slots.img");
        build(&esp, &out, "", RECORD, 512).unwrap();
        let parts = partitions(&out);
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["EFI system partition", "kai-r", "kai-a", "kai-b"]);
        assert_eq!(parts[2].kind.0, gpt::guid("kai-a partition type"));
        assert!(parts.windows(2).all(|w| w[0].last < w[1].first));
        let sizes: Vec<u64> = parts.iter().map(|p| p.sectors() / 2048).collect();
        assert_eq!(sizes, PARTITION_MIB);
        // The ESP's volume fills its partition; a slot's is its files' size, here
        // FAT32's floor of 33 MiB, and the same as the slot image of these files.
        assert_eq!(volume_sectors(&out, parts[0].first), 64 * 2048);
        let image = slot_image(&esp, RECORD).unwrap();
        assert_eq!(image.len(), 33 * MIB as usize);
        for slot in &parts[1..] {
            assert_eq!(volume_sectors(&out, slot.first), 33 * 2048, "{}", slot.name);
        }
        fs::remove_dir_all(&esp).unwrap();
    }
}
