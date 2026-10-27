//! Spike S08-1: the R/A/B disk of ADR 0008 section 1, without `kai-data`. The
//! ESP holds Limine and the constant `limine.conf`; partitions 2, 3 and 4 are
//! the slots R, A and B, each a FAT32 volume with the same kernel and bundle.
//! The config is never rewritten: the kernel picks the next boot with Limine's
//! one-shot variable, and no one-shot boots R.

use super::fat32::{self, Node};
use super::{MIB, SECTOR, gpt, volume_mib};
use std::fs;
use std::path::Path;

/// The slots' GPT names, which are also their type GUIDs' labels.
const SLOTS: [&str; 3] = ["kai-r", "kai-a", "kai-b"];
/// Volume ids, one per partition, fixed so the image is deterministic.
const VOLUME_IDS: [u32; 4] = [0x4B41_4931, 0x4B41_4952, 0x4B41_4941, 0x4B41_4942];

/// ADR 0008 section 3's `limine.conf`, with the spike's command line.
pub fn limine_conf(cmdline: &str) -> String {
    let mut conf = String::from("timeout: 0\ndefault_entry: 1\n");
    // `slot=` names the slot: Limine reports no partition for a `boot(N):` file.
    for (entry, partition, extra) in [
        ("recovery", 2, " slot=r mode=recovery"),
        ("kai-a", 3, " slot=a"),
        ("kai-b", 4, " slot=b"),
    ] {
        conf.push_str(&format!(
            "\n/{entry}\n    protocol: limine\n    path: boot({partition}):/boot/kernel\n    \
             cmdline: {cmdline}{extra}\n    module_path: boot({partition}):/boot/{}\n",
            crate::esp::BUNDLE
        ));
    }
    conf
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

/// Writes the R/A/B disk built from the directory ESP `esp` to `out`; its size.
pub fn build(esp: &Path, out: &Path, cmdline: &str) -> Result<u64, String> {
    let boot = esp.join("boot");
    let mut trees = vec![vec![
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
    ]];
    for _ in SLOTS {
        trees.push(vec![dir(
            "boot",
            vec![
                file("kernel", read(&boot.join("kernel"))?),
                file(crate::esp::BUNDLE, read(&boot.join(crate::esp::BUNDLE))?),
            ],
        )]);
    }
    let sectors: Vec<u64> = trees
        .iter()
        .map(|tree| volume_mib(content(tree)) * MIB / u64::from(SECTOR))
        .collect();
    let mut volumes = Vec::new();
    let mut first = gpt::FIRST_LBA;
    for ((tree, &count), id) in trees.iter().zip(&sectors).zip(VOLUME_IDS) {
        let count = u32::try_from(count).map_err(|_| "a partition would be too large")?;
        let hidden = u32::try_from(first).map_err(|_| "a partition starts too far in")?;
        volumes.push(fat32::volume(count, hidden, id, tree)?);
        first = (first + u64::from(count)).next_multiple_of(2048);
    }
    let mut parts = vec![gpt::Part {
        kind: gpt::ESP_TYPE,
        label: "EFI system partition",
        volume: &volumes[0],
    }];
    for (label, volume) in SLOTS.iter().zip(&volumes[1..]) {
        parts.push(gpt::Part {
            kind: gpt::guid(label),
            label,
            volume,
        });
    }
    let disk = gpt::disk_of(&parts, "kernel-ai-abr");
    fs::write(out, &disk).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(disk.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_names_every_slot_by_partition_and_defaults_to_r() {
        let conf = limine_conf("test=efi-ab");
        assert!(conf.starts_with("timeout: 0\ndefault_entry: 1\n\n/recovery\n"));
        for (entry, n) in [("recovery", 2), ("kai-a", 3), ("kai-b", 4)] {
            assert!(conf.contains(&format!("/{entry}\n")));
            assert!(conf.contains(&format!("path: boot({n}):/boot/kernel")));
            assert!(conf.contains(&format!("module_path: boot({n}):/boot/kai.bundle")));
        }
        assert!(conf.contains("cmdline: test=efi-ab slot=r mode=recovery\n"));
        assert!(conf.contains("cmdline: test=efi-ab slot=b\n"));
        assert!(!conf.contains("remember_last_entry"));
    }
}
