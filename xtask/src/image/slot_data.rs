//! The slot disk with `kai-data` on it (G58 part C): partition 5 after the
//! slots, as on a release disk, so `fs` mounts the boot disk's own `kai-data`
//! and not a disk of its own. A chained case's first boot formats it; every
//! later boot rebuilds the slots from its own build, as [`slots::build`] does,
//! and keeps `kai-data` as the last boot left it.

use super::slots::{self, DISK_NAME, Room};
use super::{MIB, gpt};
use std::fs;
use std::path::Path;

/// The boot disk's `kai-data`, in MiB: the size of a data disk of its own
/// ([`crate::blklog::DATA_SECTORS`]).
pub const DATA_MIB: u64 = crate::blklog::DATA_SECTORS / 2048;

/// Writes the slot disk of `esp` to `out`, for `sector`-byte sectors, with a
/// `kai-data` of [`DATA_MIB`]: formatted anew when `fresh` or when `out` is
/// not a disk of this layout, else kept from `out`. Its size in bytes.
pub fn build(
    esp: &Path,
    out: &Path,
    cmdline: &str,
    record: &str,
    sector: usize,
    fresh: bool,
) -> Result<u64, String> {
    let room = Room::Grow;
    let plan = slots::plan(
        esp,
        cmdline,
        record,
        Some(DATA_MIB),
        room,
        DISK_NAME,
        sector,
    )?;
    let data = plan.data.ok_or("the slot disk has no kai-data")?;
    let same = fs::metadata(out).is_ok_and(|old| old.len() == plan.bytes());
    if fresh || !same {
        slots::write(&plan, out, None)?;
        let uuid = gpt::unique(DISK_NAME, "kai-data volume");
        crate::fs_tool::format_partition(out, data.0, data.1, uuid)?;
        return Ok(plan.bytes());
    }
    // The new disk beside the old, whose kai-data it copies, then in its place.
    let next = out.with_extension("next");
    slots::write(&plan, &next, Some((out, data)))?;
    fs::rename(&next, out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(plan.bytes())
}

/// Copies the `len` bytes at `at` of the file `old` into `to`, a new file of
/// zeros, at the same place, a MiB at a time. A MiB of zeros is not written,
/// so it stays sparse in `to`.
pub(super) fn copy_range(old: &Path, to: &mut fs::File, at: u64, len: u64) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let shown = old.display();
    let mut from = fs::File::open(old).map_err(|e| format!("cannot read {shown}: {e}"))?;
    from.seek(SeekFrom::Start(at))
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    let zeros = vec![0u8; MIB as usize];
    let mut piece = vec![0u8; MIB as usize];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(MIB) as usize;
        from.read_exact(&mut piece[..n])
            .map_err(|e| format!("cannot read {shown}: {e}"))?;
        if piece[..n] != zeros[..n] {
            to.seek(SeekFrom::Start(at + done))
                .and_then(|_| to.write_all(&piece[..n]))
                .map_err(|e| format!("cannot copy bytes {at}.. from {shown}: {e}"))?;
        }
        done += n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    const RECORD: &str = "kai-slot 1\nrelease_seq 1\nepoch 1\n";

    /// A disk image file in sectors of `.1` bytes, read through `diskmap`.
    struct FileDisk(fs::File, usize);
    impl diskmap::Disk for FileDisk {
        fn sectors(&self) -> u64 {
            self.0.metadata().map_or(0, |m| m.len() / self.1 as u64)
        }
        fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
            self.0
                .seek(SeekFrom::Start(lba * self.1 as u64))
                .and_then(|_| self.0.read_exact(out))
                .map_err(|_| diskmap::Error::Read)
        }
        fn sector_size(&self) -> usize {
            self.1
        }
    }

    fn at(disk: &Path, offset: u64, bytes: Option<&[u8]>) -> Vec<u8> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(disk)
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        if let Some(bytes) = bytes {
            file.write_all(bytes).unwrap();
            return bytes.to_vec();
        }
        let mut out = vec![0; 8];
        file.read_exact(&mut out).unwrap();
        out
    }

    /// On a 4Kn disk, `kai-data` is partition 5 in 4 KiB LBAs; a chained boot
    /// keeps what the last boot wrote into it and rebuilds the slots, and a
    /// fresh one formats it again.
    #[test]
    fn kai_data_is_partition_5_and_a_chained_boot_keeps_it() {
        let dir = std::env::temp_dir().join(format!("kai-slot-data-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let esp = dir.join("esp");
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(esp.join("boot")).unwrap();
        fs::write(esp.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        fs::write(esp.join("boot/kernel"), b"kernel").unwrap();
        fs::write(esp.join("boot/kai.bundle"), b"bundle").unwrap();
        let out = dir.join("slots.img");
        build(&esp, &out, "test=files", RECORD, 4096, true).unwrap();
        let table = diskmap::read(&mut FileDisk(fs::File::open(&out).unwrap(), 4096)).unwrap();
        let parts = &table.partitions;
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[4].kind, diskmap::KAI_DATA_TYPE);
        assert_eq!(parts[4].sectors() * 4096, DATA_MIB << 20);
        assert_eq!((parts[4].first * 4096) % (1 << 20), 0);
        // A byte past kaifs's first blocks, and one in slot A's volume.
        let data = parts[4].first * 4096 + (1 << 20);
        let slot_a = parts[2].first * 4096;
        let mark = b"kept4096";
        at(&out, data, Some(mark));
        let slot_a_bytes = at(&out, slot_a, None);
        at(&out, slot_a, Some(b"scribble"));
        build(&esp, &out, "test=files reread", RECORD, 4096, false).unwrap();
        assert_eq!(at(&out, data, None), mark);
        assert_eq!(at(&out, slot_a, None), slot_a_bytes);
        build(&esp, &out, "test=files", RECORD, 4096, true).unwrap();
        assert_ne!(at(&out, data, None), mark);
        fs::remove_dir_all(dir).unwrap();
    }
}
