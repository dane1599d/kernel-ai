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
pub const PARTITION_MIB: [u64; 4] = [64, 256, 1024, 1024];
/// Volume ids, one per partition, fixed so the image is deterministic.
const VOLUME_IDS: [u32; 4] = [0x4B41_4931, 0x4B41_4952, 0x4B41_4941, 0x4B41_4942];
/// Seeds the disk's and the partitions' GUIDs. Unique per install later (the
/// installer); CI keeps this fixed seed.
const DISK_NAME: &str = "kernel-ai-slots";

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
enum Room {
    /// The release disk refuses it: an install keeps the partitions it was
    /// written with.
    Fixed,
    /// A boot case's disk grows that partition to the volume: a test build is
    /// larger than a release and is never installed.
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
/// partition's volume by its first LBA, and `kai-data`'s place if it has one.
struct Plan {
    sectors: u64,
    gpt: Vec<(u64, Vec<u8>)>,
    volumes: Vec<(u64, Vec<u8>)>,
    data: Option<(u64, u64)>,
}

impl Plan {
    /// The disk's size in bytes.
    fn bytes(&self) -> u64 {
        self.sectors * u64::from(SECTOR)
    }
}

/// The layout for the directory ESP `esp`, whose kernel and bundle every slot
/// gets, with `record` as each slot's `boot/slot`, and after the slots a
/// `kai-data` partition of `data_mib` MiB if asked for. The partitions are
/// [`PARTITION_MIB`]; `room` says what a larger volume does.
fn plan(
    esp: &Path,
    cmdline: &str,
    record: &str,
    data_mib: Option<u64>,
    room: Room,
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
            guid: diskmap::Guid(gpt::guid(&format!("{DISK_NAME} partition {name}"))),
            first,
            last,
            name,
        })
        .collect();
    let sectors = gpt::disk_sectors(first - gpt::FIRST_LBA);
    let disk_guid = diskmap::Guid(gpt::guid(&format!("{DISK_NAME} disk")));
    let gpt = diskmap::layout(sectors, disk_guid, &parts)
        .map_err(|e| format!("the slot disk does not lay out: {e:?}"))?;
    Ok(Plan {
        sectors,
        gpt,
        volumes,
        data,
    })
}

/// Writes `plan`'s disk to `out` as a new file: the GPT and each volume where
/// the layout puts it, a piece at a time, and zeros everywhere else, left
/// sparse where the system allows. The disk is gigabytes, most of them the
/// slots' free room, and is never held in memory.
fn write(plan: &Plan, out: &Path) -> Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};
    let shown = out.display();
    let fail = |e: std::io::Error| format!("cannot write {shown}: {e}");
    let mut file = fs::File::create(out).map_err(fail)?;
    file.set_len(plan.bytes()).map_err(fail)?;
    for (lba, bytes) in plan.gpt.iter().chain(&plan.volumes) {
        file.seek(SeekFrom::Start(lba * u64::from(SECTOR)))
            .and_then(|_| file.write_all(bytes))
            .map_err(fail)?;
    }
    file.sync_all().map_err(fail)
}

/// Writes the slot disk to `out`; its size. A boot case's disk: a partition
/// too small for this test build's files grows.
pub fn build(esp: &Path, out: &Path, cmdline: &str, record: &str) -> Result<u64, String> {
    let plan = plan(esp, cmdline, record, None, Room::Grow)?;
    write(&plan, out)?;
    Ok(plan.bytes())
}

/// `kai-data`'s GPT name, which is also its type GUID's label.
const DATA_NAME: &str = "kai-data";
/// The release image's `kai-data`, in MiB: room for Qwen3-4B (2.5 GB) and a
/// second model beside it, with the home folder.
pub const RELEASE_DATA_MIB: u64 = 8192;

/// Writes the release disk to `out`: the slot disk with an empty command line
/// and exactly [`PARTITION_MIB`] (a build whose files do not fit is refused),
/// then `kai-data` as partition 5, formatted with a fixed volume id, so every
/// build of a commit writes the same bytes. `updated` never writes it: it
/// writes only `kai-a` and `kai-b`. The disk is written a piece at a time and
/// left sparse where the system allows, never held in memory. Its size.
pub fn build_release(esp: &Path, out: &Path, record: &str, data_mib: u64) -> Result<u64, String> {
    let plan = plan(esp, "", record, Some(data_mib), Room::Fixed)?;
    write(&plan, out)?;
    let (first, sectors) = plan.data.ok_or("the release disk has no kai-data")?;
    let uuid = gpt::guid(&format!("{DISK_NAME} kai-data volume"));
    crate::fs_tool::format_partition(out, first, sectors, uuid)?;
    Ok(plan.bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::PathBuf;

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
        let len = build_release(&esp, &one, RECORD, 32).unwrap();
        build_release(&esp, &two, RECORD, 32).unwrap();
        assert_eq!(fs::metadata(&one).unwrap().len(), len);
        assert!(same(&one, &two, 0, len), "two builds differ");
        // Its first four partitions hold the slot disk's own bytes.
        let slots = esp.join("slots.img");
        build(&esp, &slots, "", RECORD).unwrap();
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
        let len = build_release(&now, &out, RECORD, 32).unwrap();
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
    fn the_disk_has_four_partitions_in_order_with_the_slot_types_and_the_adrs_sizes() {
        let esp = fake_esp("slots-test");
        let out = esp.join("slots.img");
        build(&esp, &out, "", RECORD).unwrap();
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
