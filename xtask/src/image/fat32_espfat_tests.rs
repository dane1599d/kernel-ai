//! `espfat` (row G43) against this writer and this reader, both ways: espfat
//! reads every file this writer lays out, and this reader reads back every
//! file espfat replaces, the long names and the partition offset included.

use super::*;

/// Where the ESP's volume starts on the test disk, as on a real one.
const START: u64 = 2048;

/// A disk in memory with the volume at [`START`], logging espfat's writes.
struct Disk {
    bytes: Vec<u8>,
    writes: Vec<(u64, [u8; 512])>,
}

impl Disk {
    fn new(volume: &[u8]) -> Self {
        let mut bytes = vec![0; START as usize * 512];
        bytes.extend_from_slice(volume);
        Self {
            bytes,
            writes: Vec::new(),
        }
    }

    fn volume(&self) -> &[u8] {
        &self.bytes[START as usize * 512..]
    }

    fn mount(&mut self) -> espfat::Volume<&mut Self> {
        let room = self.bytes.len() as u64 / 512 - START;
        espfat::Volume::open(self, START, room).unwrap()
    }
}

impl espfat::Disk for Disk {
    fn read(&mut self, lba: u64, out: &mut [u8; 512]) -> Result<(), espfat::IoError> {
        let at = lba as usize * 512;
        let sector = self.bytes.get(at..at + 512).ok_or(espfat::IoError)?;
        out.copy_from_slice(sector);
        Ok(())
    }

    fn write(&mut self, lba: u64, data: &[u8; 512]) -> Result<(), espfat::IoError> {
        let at = lba as usize * 512;
        let sector = self.bytes.get_mut(at..at + 512).ok_or(espfat::IoError)?;
        sector.copy_from_slice(data);
        self.writes.push((lba, *data));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), espfat::IoError> {
        Ok(())
    }
}

fn esp() -> Disk {
    Disk::new(&volume(131_072, START as u32, 0x1234, &esp_like()).unwrap())
}

#[test]
fn espfat_reads_every_file_this_writer_writes() {
    let mut disk = esp();
    let mut v = disk.mount();
    let read = |v: &mut espfat::Volume<_>, path: &str| v.read_file(path, usize::MAX).unwrap();
    assert_eq!(read(&mut v, "EFI/BOOT/BOOTX64.EFI"), pattern(70_000, 1));
    assert_eq!(read(&mut v, "boot/kernel"), pattern(1_500_000, 2));
    assert_eq!(read(&mut v, "boot/kernel.lines"), pattern(9_999, 3));
    // The 8.3 alias: "kernel.lines" is the third long name in boot/, after
    // "empty" and "kernel".
    assert_eq!(&short_name("kernel.lines", 3), b"KERNEL~3LIN");
    assert_eq!(
        read(&mut v, "BOOT/KERNEL~3.LIN"),
        pattern(9_999, 3),
        "the alias"
    );
    assert!(read(&mut v, "boot/empty").is_empty());
    assert_eq!(read(&mut v, "boot/limine/limine.conf"), b"timeout: 0\n");
    for i in 0..24 {
        let elf = read(&mut v, &format!("boot/user/program-{i}.elf"));
        assert_eq!(elf, pattern(3000 + i * 700, i as u8));
        let lines = read(&mut v, &format!("Boot/User/Program-{i}.Lines"));
        assert_eq!(lines, pattern(100 + i, 7));
    }
}

#[test]
fn this_reader_reads_every_file_espfat_replaces() {
    let mut disk = esp();
    let conf = b"timeout: 3\n/kai-a\n    protocol: limine\n".to_vec();
    let mut v = disk.mount();
    v.replace("EFI/BOOT/BOOTX64.EFI", &pattern(91_234, 9))
        .unwrap();
    v.replace("boot/limine/limine.conf", &conf).unwrap();
    v.replace("boot/kernel", &pattern(4_000, 8)).unwrap();
    v.replace("boot/empty", &pattern(777, 6)).unwrap();
    v.replace("boot/kernel.lines", &[]).unwrap();
    // The reader checks that both FATs match before it reads anything.
    let reader = Reader::new(disk.volume());
    assert_eq!(reader.open("EFI/BOOT/BOOTX64.EFI"), pattern(91_234, 9));
    assert_eq!(reader.open("boot/limine/limine.conf"), conf);
    assert_eq!(reader.open("boot/kernel"), pattern(4_000, 8));
    assert_eq!(reader.open("boot/empty"), pattern(777, 6));
    assert!(reader.open("boot/kernel.lines").is_empty());
    for i in 0..24 {
        let elf = reader.open(&format!("boot/user/program-{i}.elf"));
        assert_eq!(elf, pattern(3000 + i * 700, i as u8));
    }
    // Names, attributes and the boot sector are as this writer left them.
    let fresh = volume(131_072, START as u32, 0x1234, &esp_like()).unwrap();
    assert_eq!(disk.volume()[..512], fresh[..512]);
    let names = |r: &Reader, at| {
        r.list(at)
            .into_iter()
            .map(|e| (e.0, e.1))
            .collect::<Vec<_>>()
    };
    let old = Reader::new(&fresh);
    assert_eq!(names(&reader, reader.root), names(&old, old.root));
}

/// A cut after any of espfat's writes leaves the old file or the new one,
/// through both FAT copies; this reader reads the finished volume.
#[test]
fn a_cut_replace_on_this_writers_volume_is_old_or_new() {
    for (path, new) in [
        ("boot/limine/limine.conf", b"timeout: 5\n".to_vec()),
        ("EFI/BOOT/BOOTX64.EFI", pattern(80_000, 5)),
    ] {
        let mut disk = esp();
        let old = disk.mount().read_file(path, usize::MAX).unwrap();
        let before = disk.bytes.clone();
        disk.mount().replace(path, &new).unwrap();
        let writes = std::mem::take(&mut disk.writes);
        let mut cut = Disk {
            bytes: before,
            writes: Vec::new(),
        };
        let mut switched = false;
        for (k, (lba, data)) in writes.iter().enumerate() {
            let at = *lba as usize * 512;
            cut.bytes[at..at + 512].copy_from_slice(data);
            let mut seen = Vec::new();
            for copy in 0..2 {
                let mut v = cut.mount();
                v.read_through(copy).unwrap();
                let got = v.read_file(path, usize::MAX).unwrap();
                assert!(
                    got == old || got == new,
                    "{path} after write {k}, FAT {copy}"
                );
                seen.push(got == new);
                let kernel = v.read_file("boot/kernel", usize::MAX).unwrap();
                assert_eq!(kernel, pattern(1_500_000, 2));
            }
            assert_eq!(seen[0], seen[1], "{path} after write {k}");
            assert!(
                !switched || seen[0],
                "{path} went back to the old file at {k}"
            );
            switched = seen[0];
        }
        assert!(switched);
        let reader = Reader::new(cut.volume());
        assert_eq!(reader.open(path), new);
    }
}
