//! The slot disk of ADR 0008 section 1: the ESP with Limine and the constant
//! `limine.conf`, then `kai-r`, `kai-a` and `kai-b`, each a FAT32 volume with
//! the kernel, the bundle and the slot's `kai-slot 1` (`boot/slot`). The config
//! is never rewritten: the kernel picks the next boot with Limine's one-shot
//! variable (`kernel/src/slots.rs`), and no one-shot boots R.
//!
//! `kai-data` (partition 5) joins once `fs` mounts it from the boot disk; until
//! then, cases attach it as a disk of its own, as today.

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

/// The disk for the directory ESP `esp`, whose kernel and bundle every slot
/// gets, with `record` as each slot's `boot/slot`.
pub fn disk(esp: &Path, cmdline: &str, record: &str) -> Result<Vec<u8>, String> {
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
        trees.push(vec![dir(
            "boot",
            vec![
                file("kernel", read(&boot.join("kernel"))?),
                file(crate::esp::BUNDLE, read(&boot.join(crate::esp::BUNDLE))?),
                file("slot", record.as_bytes().to_vec()),
            ],
        )]);
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
    let mut disk = gpt::laid_out(sectors, &gpt::guid(&format!("{DISK_NAME} disk")), &parts);
    for (lba, volume) in volumes {
        let at = (lba * u64::from(SECTOR)) as usize;
        disk[at..at + volume.len()].copy_from_slice(&volume);
    }
    Ok(disk)
}

/// Writes the slot disk to `out`; its size.
pub fn build(esp: &Path, out: &Path, cmdline: &str, record: &str) -> Result<u64, String> {
    let disk = disk(esp, cmdline, record)?;
    fs::write(out, &disk).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(disk.len() as u64)
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

    #[test]
    fn the_disk_has_four_partitions_in_order_with_the_slot_types() {
        let esp = std::env::temp_dir().join(format!("kai-slots-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&esp);
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(esp.join("boot")).unwrap();
        fs::write(esp.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        fs::write(esp.join("boot/kernel"), b"kernel").unwrap();
        fs::write(esp.join("boot/kai.bundle"), b"bundle").unwrap();
        let disk = disk(&esp, "", "kai-slot 1\nrelease_seq 1\nepoch 1\n").unwrap();
        fs::remove_dir_all(&esp).unwrap();
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
        let parts = diskmap::read(&mut Mem(disk)).unwrap().partitions;
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["EFI system partition", "kai-r", "kai-a", "kai-b"]);
        assert_eq!(parts[2].kind.0, gpt::guid("kai-a partition type"));
        assert!(parts.windows(2).all(|w| w[0].last < w[1].first));
    }
}
