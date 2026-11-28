//! The slot disk with `kai-data` on it (G58 part C): partition 5 after the
//! slots, as on a release disk, so `fs` mounts the boot disk's own `kai-data`
//! and not a disk of its own. A chained case's first boot formats it; every
//! later boot rebuilds the slots from its own build, as [`slots::build`] does,
//! and keeps `kai-data` as the last boot left it.

use super::slots::{self, DISK_NAME, Keep, Room};
use super::{MIB, gpt};
use std::fs;
use std::path::Path;

/// The boot disk's `kai-data`, in MiB: the size of a data disk of its own
/// ([`crate::blklog::DATA_SECTORS`]).
pub const DATA_MIB: u64 = crate::blklog::DATA_SECTORS / 2048;

/// Writes the slot disk of `esp` to `out`, for `sector`-byte sectors, with a
/// `kai-data` of [`DATA_MIB`]: formatted anew when `fresh`, else kept from
/// `out`, which must be a disk of this layout's length (a chain's earlier
/// boot); a missing or other disk is an error, never a silent new format.
/// Its size in bytes.
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
    if fresh {
        slots::write(&plan, out, None)?;
        let uuid = gpt::unique(DISK_NAME, "kai-data volume");
        crate::fs_tool::format_partition(out, data.0, data.1, uuid)?;
        return Ok(plan.bytes());
    }
    let shown = out.display();
    let old = fs::metadata(out).map_err(|e| format!("cannot keep kai-data: {shown}: {e}"))?;
    if old.len() != plan.bytes() {
        return Err(format!(
            "cannot keep kai-data: {shown} is {} bytes, not this layout's {}",
            old.len(),
            plan.bytes()
        ));
    }
    // The new disk beside the old, whose kai-data it copies, then in its place.
    let next = out.with_extension("next");
    slots::write(&plan, &next, Some((out, Keep::Data)))?;
    fs::rename(&next, out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(plan.bytes())
}

/// Copies the `len` bytes at `at` of the file `old` into `to`, a new file of
/// zeros, at the same place, a MiB at a time. A MiB of zeros is not written,
/// so it stays sparse in `to`. Only `slots::write` calls it, with the range
/// `Plan::kept` works out for its [`Keep`]: no other range of an old disk is
/// ever copied.
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

    /// A directory ESP in a new `dir`, as `slots::plan` reads one.
    fn fake_esp(dir: &Path) -> std::path::PathBuf {
        let _ = fs::remove_dir_all(dir);
        let esp = dir.join("esp");
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(esp.join("boot")).unwrap();
        fs::write(esp.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        fs::write(esp.join("boot/kernel"), b"kernel").unwrap();
        fs::write(esp.join("boot/kai.bundle"), b"bundle").unwrap();
        esp
    }

    fn table(disk: &Path, sector: usize) -> Vec<diskmap::Partition> {
        let mut disk = FileDisk(fs::File::open(disk).unwrap(), sector);
        diskmap::read(&mut disk).unwrap().partitions
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
        let esp = fake_esp(&dir);
        let out = dir.join("slots.img");
        build(&esp, &out, "test=files", RECORD, 4096, true).unwrap();
        let parts = &table(&out, 4096);
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
        // A chained boot with no earlier disk, or one of another length.
        let other = dir.join("other.img");
        let err = build(&esp, &other, "test=files", RECORD, 4096, false).unwrap_err();
        assert!(err.starts_with("cannot keep kai-data"), "{err}");
        fs::write(&other, b"short").unwrap();
        let err = build(&esp, &other, "test=files", RECORD, 4096, false).unwrap_err();
        assert!(err.contains("is 5 bytes, not this layout's"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// The slot disk without `kai-data` (G58 B), at 512 and at 4096: the same
    /// length, the same bytes outside the two 4Kn GPTs (which cover the 512
    /// ones), and the same four partitions, each at the same byte offset, read
    /// in the disk's own LBAs.
    #[test]
    fn a_4kn_slot_disk_is_the_512_disk_but_its_gpt() {
        use std::io::{Read, Seek, SeekFrom};
        let dir = std::env::temp_dir().join(format!("kai-slot-4kn-{}", std::process::id()));
        let esp = fake_esp(&dir);
        let (small, big) = (dir.join("512.img"), dir.join("4096.img"));
        slots::build(&esp, &small, "test=slot", RECORD, 512).unwrap();
        slots::build(&esp, &big, "test=slot", RECORD, 4096).unwrap();
        let bytes = |parts: &[diskmap::Partition], sector: u64| -> Vec<(u64, u64)> {
            parts
                .iter()
                .map(|p| (p.first * sector, p.sectors() * sector))
                .collect()
        };
        let parts = bytes(&table(&small, 512), 512);
        assert_eq!(parts.len(), 4);
        assert_eq!(bytes(&table(&big, 4096), 4096), parts);
        let len = fs::metadata(&small).unwrap().len();
        assert_eq!(fs::metadata(&big).unwrap().len(), len);
        // Every byte from the 4Kn primary GPT's end (its MBR, header and 4
        // LBAs of entries) to the 4Kn backup GPT's start (4 LBAs and a header).
        let (start, end) = (6 * 4096, len - 5 * 4096);
        assert!(start <= parts[0].0 && parts[3].0 + parts[3].1 <= end);
        let mut files = [&small, &big].map(|p| fs::File::open(p).unwrap());
        let mut pieces = [vec![0u8; 1 << 20], vec![0u8; 1 << 20]];
        let mut offset = start;
        while offset < end {
            let n = (end - offset).min(1 << 20) as usize;
            for (file, piece) in files.iter_mut().zip(pieces.iter_mut()) {
                file.seek(SeekFrom::Start(offset)).unwrap();
                file.read_exact(&mut piece[..n]).unwrap();
            }
            assert!(pieces[0][..n] == pieces[1][..n], "bytes differ at {offset}");
            offset += n as u64;
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
