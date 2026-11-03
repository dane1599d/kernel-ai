//! The slot disk of ADR 0008 section 1: the ESP with Limine and the constant
//! `limine.conf`, then `kai-r`, `kai-a` and `kai-b`, each a FAT32 volume with
//! the kernel, the bundle and the slot's `kai-slot 1` (`boot/slot`). The config
//! is never rewritten: the kernel picks the next boot with Limine's one-shot
//! variable (`kernel/src/slots.rs`), and no one-shot boots R.
//!
//! The release disk adds `kai-data` as partition 5 ([`build_release`]), which
//! `fs` finds by its type as it finds a disk of its own; the boot cases attach
//! `kai-data` as a disk of its own.

use super::fat32::{self, Node};
use super::{MIB, SECTOR, gpt, volume_mib};
use std::fs;
use std::path::Path;

/// The slots' GPT names, which are also their type GUIDs' labels.
pub const SLOTS: [&str; 3] = ["kai-r", "kai-a", "kai-b"];
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
/// and the bundle's `kai.slot`.
pub fn slot_record(root: &Path, release_seq: u32) -> String {
    let version = fs::read_to_string(root.join("kernel/Cargo.toml"))
        .ok()
        .and_then(|toml| {
            toml.lines().find_map(|line| {
                line.strip_prefix("version = ")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| "0.0.0".into());
    format!(
        "kai-slot 1\nrelease_seq {release_seq}\nepoch 1\nversion {version}\ncommit {}\nchannel dev\n",
        crate::esp::commit_id(root)
    )
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

fn content(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|node| match node {
            Node::File { bytes, .. } => bytes.len() as u64,
            Node::Dir { children, .. } => content(children),
        })
        .sum()
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
/// a slot partition, as an update carries it. It must fit the slot it lands in,
/// which the disk sized for the same files.
pub fn slot_image(esp: &Path, record: &str) -> Result<Vec<u8>, String> {
    let tree = slot_tree(&esp.join("boot"), record)?;
    let count = volume_mib(content(&tree)) * MIB / u64::from(SECTOR);
    let sectors = u32::try_from(count).map_err(|_| "a slot image would be too large")?;
    fat32::volume(sectors, 0, VOLUME_IDS[2], &tree)
}

/// A slot disk laid out: its size in sectors, its GPT's sectors, each
/// partition's volume by its first LBA, and `kai-data`'s place if it has one.
struct Plan {
    sectors: u64,
    gpt: Vec<(u64, Vec<u8>)>,
    volumes: Vec<(u64, Vec<u8>)>,
    data: Option<(u64, u64)>,
}

/// The layout for the directory ESP `esp`, whose kernel and bundle every slot
/// gets, with `record` as each slot's `boot/slot`, and after the slots a
/// `kai-data` partition of `data_mib` MiB if asked for.
fn plan(esp: &Path, cmdline: &str, record: &str, data_mib: Option<u64>) -> Result<Plan, String> {
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
    for (n, (tree, id)) in trees.iter().zip(VOLUME_IDS).enumerate() {
        let count = volume_mib(content(tree)) * MIB / u64::from(SECTOR);
        let sectors = u32::try_from(count).map_err(|_| "a partition would be too large")?;
        let hidden = u32::try_from(first).map_err(|_| "a partition starts too far in")?;
        volumes.push((first, fat32::volume(sectors, hidden, id, tree)?));
        let (kind, name) = match n {
            0 => (diskmap::ESP_TYPE, "EFI system partition"),
            _ => (
                diskmap::Guid(gpt::guid(&format!("{} partition type", SLOTS[n - 1]))),
                SLOTS[n - 1],
            ),
        };
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

/// The disk for the directory ESP `esp`, whose kernel and bundle every slot
/// gets, with `record` as each slot's `boot/slot`.
pub fn disk(esp: &Path, cmdline: &str, record: &str) -> Result<Vec<u8>, String> {
    let plan = plan(esp, cmdline, record, None)?;
    let mut disk = vec![0u8; (plan.sectors * u64::from(SECTOR)) as usize];
    for (lba, bytes) in plan.gpt.iter().chain(&plan.volumes) {
        let at = (lba * u64::from(SECTOR)) as usize;
        disk[at..at + bytes.len()].copy_from_slice(bytes);
    }
    Ok(disk)
}

/// Writes the slot disk to `out`; its size.
pub fn build(esp: &Path, out: &Path, cmdline: &str, record: &str) -> Result<u64, String> {
    let disk = disk(esp, cmdline, record)?;
    fs::write(out, &disk).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(disk.len() as u64)
}

/// `kai-data`'s GPT name, which is also its type GUID's label.
const DATA_NAME: &str = "kai-data";
/// The release image's `kai-data`, in MiB: room for Qwen3-4B (2.5 GB) and a
/// second model beside it, with the home folder.
pub const RELEASE_DATA_MIB: u64 = 8192;

/// Writes the release disk to `out`: the slot disk with an empty command line,
/// then `kai-data` as partition 5, formatted with a fixed volume id, so every
/// build of a commit writes the same bytes. `updated` never writes it: it
/// writes only `kai-a` and `kai-b`. The disk is written a piece at a time and
/// left sparse where the system allows, never held in memory. Its size.
pub fn build_release(esp: &Path, out: &Path, record: &str, data_mib: u64) -> Result<u64, String> {
    use std::io::{Seek, SeekFrom, Write};
    let plan = plan(esp, "", record, Some(data_mib))?;
    let shown = out.display();
    let len = plan.sectors * u64::from(SECTOR);
    let mut file = fs::File::create(out).map_err(|e| format!("cannot write {shown}: {e}"))?;
    file.set_len(len)
        .map_err(|e| format!("cannot size {shown}: {e}"))?;
    for (lba, bytes) in plan.gpt.iter().chain(&plan.volumes) {
        file.seek(SeekFrom::Start(lba * u64::from(SECTOR)))
            .and_then(|_| file.write_all(bytes))
            .map_err(|e| format!("cannot write {shown}: {e}"))?;
    }
    file.sync_all()
        .map_err(|e| format!("cannot write {shown}: {e}"))?;
    drop(file);
    let (first, sectors) = plan.data.ok_or("the release disk has no kai-data")?;
    let uuid = gpt::guid(&format!("{DISK_NAME} kai-data volume"));
    crate::fs_tool::format_partition(out, first, sectors, uuid)?;
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_slot_record_reads_back_with_bootslot() {
        let record = slot_record(Path::new("."), 5);
        let file = bootslot::slotfile::parse(&record).unwrap();
        assert_eq!((file.release_seq, file.epoch, file.channel), (5, 1, "dev"));
    }

    const RECORD: &str = "kai-slot 1\nrelease_seq 1\nepoch 1\n";

    /// A directory ESP with stand-in files, under the system's temp directory.
    fn fake_esp(name: &str) -> std::path::PathBuf {
        let esp = std::env::temp_dir().join(format!("kai-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&esp);
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(esp.join("boot")).unwrap();
        fs::write(esp.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        fs::write(esp.join("boot/kernel"), b"kernel").unwrap();
        fs::write(esp.join("boot/kai.bundle"), b"bundle").unwrap();
        esp
    }

    struct Mem(Vec<u8>);
    impl diskmap::Disk for Mem {
        fn sectors(&self) -> u64 {
            self.0.len() as u64 / 512
        }
        fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
            let at = lba as usize * 512;
            out.copy_from_slice(&self.0[at..at + out.len()]);
            Ok(())
        }
    }

    #[test]
    fn the_release_disk_adds_a_kai_data_that_checks_and_builds_the_same_bytes_twice() {
        let esp = fake_esp("release-disk");
        let (one, two) = (esp.join("one.img"), esp.join("two.img"));
        let len = build_release(&esp, &one, RECORD, 32).unwrap();
        build_release(&esp, &two, RECORD, 32).unwrap();
        let bytes = fs::read(&one).unwrap();
        assert_eq!(bytes.len() as u64, len);
        assert!(bytes == fs::read(&two).unwrap(), "two builds differ");
        // Its first four partitions hold the slot disk's own bytes.
        let slots = disk(&esp, "", RECORD).unwrap();
        let parts = diskmap::read(&mut Mem(bytes.clone())).unwrap().partitions;
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
        let (from, to) = (2048 * 512, (parts[3].last + 1) as usize * 512);
        assert!(bytes[from..to] == slots[from..to]);
        // kaifs on it mounts and checks, with its first folders.
        let path = one.display().to_string();
        crate::fs_tool::main(&["check".into(), path.clone()]).unwrap();
        crate::fs_tool::main(&["ls".into(), path, "/home/user".into()]).unwrap();
        fs::remove_dir_all(&esp).unwrap();
    }

    #[test]
    fn the_disk_has_four_partitions_in_order_with_the_slot_types() {
        let esp = fake_esp("slots-test");
        let disk = disk(&esp, "", RECORD).unwrap();
        fs::remove_dir_all(&esp).unwrap();
        let parts = diskmap::read(&mut Mem(disk)).unwrap().partitions;
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["EFI system partition", "kai-r", "kai-a", "kai-b"]);
        assert_eq!(parts[2].kind.0, gpt::guid("kai-a partition type"));
        assert!(parts.windows(2).all(|w| w[0].last < w[1].first));
    }
}
