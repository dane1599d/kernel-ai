//! The installer (G42) against xtask's own release image: an install lays
//! out as `release-image --image-id` does, and installing from a real release
//! disk, with its FAT32 volumes and `boot/slot` records, gives a disk that
//! reads back and whose kai-data checks.

use super::slots::{DISK_NAME, build_release};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const RECORD: &str = "kai-slot 1\nrelease_seq 4\nepoch 1\n";

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

/// A disk image file, a few sectors at a time: the disks are gigabytes, and
/// sparse.
struct FileDisk(fs::File);

impl installer::Disk for FileDisk {
    fn sectors(&self) -> u64 {
        self.0.metadata().map_or(0, |m| m.len() / 512)
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), installer::IoError> {
        self.0
            .seek(SeekFrom::Start(lba * 512))
            .and_then(|_| self.0.read_exact(out))
            .map_err(|_| installer::IoError)
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), installer::IoError> {
        self.0
            .seek(SeekFrom::Start(lba * 512))
            .and_then(|_| self.0.write_all(data))
            .map_err(|_| installer::IoError)
    }

    fn flush(&mut self) -> Result<(), installer::IoError> {
        self.0.sync_data().map_err(|_| installer::IoError)
    }
}

fn open(path: &Path) -> FileDisk {
    FileDisk(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap(),
    )
}

/// The SHA-256 of the FAT32 volume at `lba`, as many sectors as its boot
/// sector says.
fn volume_sha256(disk: &mut FileDisk, lba: u64) -> [u8; 32] {
    use installer::Disk;
    let mut boot = [0u8; 512];
    disk.read(lba, &mut boot).unwrap();
    let sectors = u64::from(u32::from_le_bytes(boot[32..36].try_into().unwrap()));
    let mut h = Sha256::new();
    let mut piece = vec![0u8; 512 * 128];
    let mut done = 0;
    while done < sectors {
        let n = (sectors - done).min(128);
        let bytes = &mut piece[..n as usize * 512];
        disk.read(lba + done, bytes).unwrap();
        h.update(&*bytes);
        done += n;
    }
    h.finalize().into()
}

/// An install with an id lays out the disk as a release image with that id
/// does, GUIDs and all; only kai-data takes the target's rest.
#[test]
fn an_install_lays_out_as_a_release_image_with_its_id() {
    let esp = fake_esp("install-layout");
    let out = esp.join("release.img");
    let len = build_release(&esp, &out, RECORD, 32, "install-x").unwrap();
    let release = installer::current(&mut open(&out)).unwrap();
    let target = installer::Target {
        sectors: len / 512,
        sector_size: 512,
        id: "install-x",
        now: 0,
    };
    let source = installer::Source {
        table: &release,
        committed: installer::Committed {
            kind: diskmap::KAI_A_TYPE,
            sha256: [0; 32],
            release_seq: 4,
        },
    };
    let plan = installer::plan(target, source, None).unwrap();
    assert_eq!(plan.disk_guid, release.disk_guid);
    for (n, (want, got)) in release.partitions.iter().zip(&plan.parts).enumerate() {
        assert_eq!(
            (want.kind, want.guid, want.first),
            (got.kind, got.guid, got.first)
        );
        assert_eq!(want.name, got.name);
        if n < 4 {
            assert_eq!(want.last, got.last, "{}", want.name);
        }
    }
    fs::remove_dir_all(&esp).unwrap();
}

/// Installing from a real release disk, committed slot B, to a blank file: the
/// copies check out through espfat and `boot/slot`, the table reads back, and
/// `fs check` passes on the new kai-data.
#[test]
fn an_install_from_a_release_disk_reads_back_and_checks() {
    let esp = fake_esp("install-apply");
    let from = esp.join("release.img");
    let len = build_release(&esp, &from, RECORD, 32, DISK_NAME).unwrap();
    let mut source = open(&from);
    let table = installer::current(&mut source).unwrap();
    let b = table.find(diskmap::KAI_B_TYPE).unwrap().first;
    let committed = installer::Committed {
        kind: diskmap::KAI_B_TYPE,
        sha256: volume_sha256(&mut source, b),
        release_seq: 4,
    };
    let to = esp.join("installed.img");
    let sectors = len / 512 + 2048 * 16;
    fs::File::create(&to)
        .unwrap()
        .set_len(sectors * 512)
        .unwrap();
    let mut target = open(&to);
    let now = installer::current(&mut target);
    let t = installer::Target {
        sectors,
        sector_size: 512,
        id: "pc-1",
        now: 0,
    };
    let src = installer::Source {
        table: &table,
        committed,
    };
    let plan = installer::plan(t, src, now.as_ref()).unwrap();
    installer::apply(&plan, &mut source, &mut target).unwrap();
    let got = installer::current(&mut target).unwrap();
    assert!(!got.from_backup);
    assert_eq!(got.disk_guid, plan.disk_guid);
    assert_eq!(got.partitions.len(), 5);
    let a = got.find(diskmap::KAI_A_TYPE).unwrap().first;
    assert_eq!(volume_sha256(&mut target, a), committed.sha256);
    crate::fs_tool::main(&["check".into(), to.display().to_string()]).unwrap();
    // A wrong release for the slot stops it before the table.
    let wrong = installer::Source {
        table: &table,
        committed: installer::Committed {
            release_seq: 5,
            ..committed
        },
    };
    let again = esp.join("again.img");
    fs::File::create(&again)
        .unwrap()
        .set_len(sectors * 512)
        .unwrap();
    let plan = installer::plan(t, wrong, None).unwrap();
    let refused = installer::apply(&plan, &mut source, &mut open(&again));
    assert!(
        matches!(refused, Err(installer::Error::Verify(_))),
        "{refused:?}"
    );
    assert_eq!(installer::current(&mut open(&again)), None);
    fs::remove_dir_all(&esp).unwrap();
}
