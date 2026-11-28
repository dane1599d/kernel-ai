//! The boot disk of a case: the directory ESP, or a disk image in its place
//! (the image case, the slot disk, a disk an update case names), and the
//! read-only update and import disks.

use super::ESP_DRIVE;
use crate::image;
use std::path::Path;

/// The image case: builds the GPT disk image of `esp` as `target/boot.img` and
/// boots from it in place of the directory ESP.
pub fn boot_from_image(args: &mut [String], esp: &Path, target_dir: &Path) -> Result<(), String> {
    let bytes = crate::image::build(esp, &target_dir.join(IMAGE))?;
    println!("xtask: boot image {IMAGE}, {} MiB", bytes >> 20);
    let drive = args
        .iter_mut()
        .find(|arg| *arg == ESP_DRIVE)
        .ok_or("no directory ESP drive to replace")?;
    *drive = format!("format=raw,file={IMAGE}");
    Ok(())
}

/// The slot cases: builds the R/A/B disk of `esp` (ADR 0008) as
/// `target/slots.img` and boots from it in place of the directory ESP.
/// `fresh` is a chain's first boot: with `boot.data`, it formats `kai-data`
/// and every later boot keeps it. With `boot.keep_b`, slot B keeps what the
/// last boot left (a resumed download).
pub fn boot_from_slots(
    args: &mut Vec<String>,
    esp: &Path,
    target_dir: &Path,
    cmdline: &str,
    record: &str,
    fresh: bool,
    boot: BootDisk,
) -> Result<(), String> {
    let out = target_dir.join(SLOTS_IMAGE);
    let sector = if boot.four_k { 4096 } else { 512 };
    let bytes = match (boot.data, boot.keep_b) {
        (true, true) => return Err("a slot disk with kai-data does not keep slot B".into()),
        (true, false) => image::slot_data::build(esp, &out, cmdline, record, sector, fresh)?,
        (false, true) if boot.four_k => return Err("a 4Kn slot disk does not keep slot B".into()),
        (false, true) => image::slots::build_keeping_b(esp, &out, cmdline, record)?,
        (false, false) => image::slots::build(esp, &out, cmdline, record, sector)?,
    };
    let on = if boot.nvme { "NVMe" } else { "virtio-blk" };
    println!(
        "xtask: slot disk {SLOTS_IMAGE}, {} MiB, {sector}-byte sectors on {on}",
        bytes >> 20
    );
    boot_from_disk_on(args, SLOTS_IMAGE, boot)
}

/// The boot disk's device (G58): virtio-blk or NVMe, of 512-byte or 4 KiB blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootDisk {
    pub nvme: bool,
    pub four_k: bool,
    /// `kai-data` as partition 5 (G58 part C), not a disk of its own.
    pub data: bool,
    /// Slot B kept from the last boot's disk (a resumed download).
    pub keep_b: bool,
}

impl BootDisk {
    /// The `-device` for the drive `slots`, serial `kai-boot`, booted first.
    fn device(self) -> String {
        let kind = if self.nvme { "nvme" } else { "virtio-blk-pci" };
        let blocks = [crate::disk::BLOCKS_4K, ""][usize::from(!self.four_k)];
        format!("{kind},drive=slots,serial=kai-boot,bootindex=0{blocks}")
    }
}

/// Boots from the disk image `name` (relative to `target/`, or absolute) in
/// place of the directory ESP, on virtio-blk, so the kernel reaches the slots
/// (`updated`'s SLOT_* calls) as an installed system reaches its one disk.
pub fn boot_from_disk(args: &mut Vec<String>, name: &str) -> Result<(), String> {
    boot_from_disk_on(args, name, BootDisk::default())
}

/// [`boot_from_disk`] on the device `boot` names.
pub fn boot_from_disk_on(args: &mut Vec<String>, name: &str, boot: BootDisk) -> Result<(), String> {
    let at = args.iter().position(|arg| arg == ESP_DRIVE);
    let at = at.ok_or("no directory ESP drive to replace")?;
    args[at] = format!("if=none,id=slots,format=raw,file={name}");
    args.splice(at + 1..at + 1, ["-device".to_string(), boot.device()]);
    Ok(())
}

/// The update disk `name` in `target/`, read-only, found by its serial.
pub fn update_disk_args(name: &str) -> [String; 4] {
    [
        "-drive".into(),
        format!("if=none,id=update,format=raw,readonly=on,file={name}"),
        "-device".into(),
        "virtio-blk-pci,drive=update,serial=kai-update".into(),
    ]
}

/// QEMU arguments attaching `name`, a file in `target/` as the update disk is,
/// read-only as the import disk, by its serial (`kabi::update::IMPORT_SERIAL`).
pub fn import_disk_args(name: &str) -> [String; 4] {
    [
        "-drive".into(),
        format!("if=none,id=import,format=raw,readonly=on,file={name}"),
        "-device".into(),
        format!(
            "virtio-blk-pci,drive=import,serial={}",
            kabi::update::IMPORT_SERIAL
        ),
    ]
}

/// The slot cases' disk, in `target/`.
const SLOTS_IMAGE: &str = "slots.img";

/// The image case's disk, in `target/`.
const IMAGE: &str = "boot.img";

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot disk with `kai-data` keeps that, never slot B too: the
    /// refusal comes before any disk is built.
    #[test]
    fn kai_data_and_a_kept_slot_b_are_refused_together() {
        let boot = BootDisk {
            four_k: true,
            data: true,
            keep_b: true,
            ..Default::default()
        };
        let none = Path::new("no-such-dir");
        let err = boot_from_slots(&mut Vec::new(), none, none, "", "", false, boot).unwrap_err();
        assert_eq!(err, "a slot disk with kai-data does not keep slot B");
    }
}
