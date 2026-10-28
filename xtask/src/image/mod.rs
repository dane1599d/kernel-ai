//! The bootable disk image: a GPT disk whose one partition is a FAT32 EFI system
//! partition holding exactly what the directory ESP (`esp.rs`) holds, Limine, the
//! kernel and its modules. VirtualBox boots it (as a VDI), QEMU's `image` case
//! boots it raw, and P7's USB stick is the same image written to a device.

pub mod fat32;
pub mod gpt;
pub mod slots;
pub mod update_disk;

use fat32::Node;
use std::fs;
use std::path::Path;

pub const SECTOR: u32 = 512;
pub(crate) const MIB: u64 = 1024 * 1024;
/// The smallest ESP written: comfortably above FAT32's 32.5 MiB floor.
const MIN_VOLUME_MIB: u64 = 64;
/// Fixed, so the same files give the same image.
const VOLUME_ID: u32 = 0x4B41_4931;
/// Seeds the disk's and the partition's GUIDs.
const DISK_NAME: &str = "kernel-ai";

/// The files and directories under `dir`, as the FAT32 writer takes them.
pub fn tree(dir: &Path) -> Result<Vec<Node>, String> {
    let mut nodes = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("{name:?} in {} is not UTF-8", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            nodes.push(Node::Dir {
                name,
                children: tree(&path)?,
            });
        } else {
            let bytes =
                fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            nodes.push(Node::File { name, bytes });
        }
    }
    Ok(nodes)
}

fn content_bytes(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|node| match node {
            Node::File { bytes, .. } => bytes.len() as u64,
            Node::Dir { children, .. } => content_bytes(children),
        })
        .sum()
}

/// The ESP's size for `content` bytes of files: room for a quarter more and
/// 16 MiB besides (cluster slack, the FATs, the next build), in whole MiB.
pub fn volume_mib(content: u64) -> u64 {
    (content + content / 4 + 16 * MIB)
        .div_ceil(MIB)
        .max(MIN_VOLUME_MIB)
}

/// The whole disk for `nodes`.
pub fn disk(nodes: &[Node]) -> Result<Vec<u8>, String> {
    let sectors = volume_mib(content_bytes(nodes)) * MIB / u64::from(SECTOR);
    let sectors = u32::try_from(sectors).map_err(|_| "the ESP would be too large")?;
    let hidden = gpt::FIRST_LBA as u32;
    let volume = fat32::volume(sectors, hidden, VOLUME_ID, nodes)?;
    Ok(gpt::disk(&volume, DISK_NAME))
}

/// Writes the bootable image of the directory ESP `esp` to `out`; returns its size.
pub fn build(esp: &Path, out: &Path) -> Result<u64, String> {
    let disk = disk(&tree(esp)?)?;
    fs::write(out, &disk).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(disk.len() as u64)
}
