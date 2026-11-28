//! Disks of 4 KiB blocks (G58, part A): the data disk and the model disk on
//! NVMe and on virtio-blk with `logical_block_size=4096`. `kai-data` on kaifs
//! takes files through `fs` on boot 1 and reads them back on boot 2, and the
//! model is read whole from its own disk on both boots. The boot disk keeps
//! 512-byte sectors there; part B moves the slots to 4 KiB, and part C puts
//! the whole system there: `kai-data` on the boot disk, and `updated`
//! writing a slot of it.

use super::{TestCase, UpdateDisk, case};
use crate::blklog::Data;
use crate::disk::Fixture;

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};
const NVME: Data = Data {
    nvme: true,
    ..VIRTIO
};

/// The fixture model's copy, on both boots of both cases.
const MODEL: &str = "model: 2101245 bytes of GGUF v3 copied from the kai-model disk";
/// The model disk's own line (`block::register`): the fixture padded to whole
/// 4 KiB blocks, read in them. No other disk of these cases has its size.
const MODEL_DISK: &str = "4104 sectors (2052 KiB, 4096-byte blocks)";

/// A boot of a 4 KiB case: the data disk `data`, the model disk on NVMe when
/// `nvme`, both of 4 KiB blocks.
const fn four_k(
    name: &'static str,
    cmdline: &'static str,
    data: Data,
    nvme: bool,
    expect: &'static [&'static str],
) -> TestCase {
    TestCase {
        expect,
        server: false,
        data: Some(data),
        disk: Some(Fixture::Gguf),
        nvme,
        four_k: true,
        ..case(name, cmdline)
    }
}

const NVME_2: TestCase = four_k(
    "nvme-4k (boot 2)",
    "test=files reread",
    NVME,
    true,
    &[
        "blocks of 4096 bytes",
        "4096-byte blocks), writable",
        MODEL,
        "filecheck: boot 2 read back what boot 1 wrote, byte for byte",
        "files: boot 2: filecheck's step through its Dir handle passed",
    ],
);
const BLK_2: TestCase = four_k(
    "blk-4k (boot 2)",
    "test=files reread",
    VIRTIO,
    false,
    &[
        "4096-byte blocks), writable",
        MODEL,
        "filecheck: boot 2 read back what boot 1 wrote, byte for byte",
        "files: boot 2: filecheck's step through its Dir handle passed",
    ],
);

/// Part B: a slot disk of 4 KiB sectors, its GPT in 4 KiB LBAs and its FAT
/// volumes' BPBs still 512 bytes (decision row `2026-11-27-4kn-esp`), booted
/// by OVMF and Limine from virtio-blk or NVMe. Boot 1 is R installing and
/// arming A; boot 2 is A, which commits.
const fn slot(
    name: &'static str,
    cmdline: &'static str,
    nvme: bool,
    expect: &'static [&'static str],
) -> TestCase {
    TestCase {
        expect,
        server: false,
        resets: true,
        slots: true,
        four_k: true,
        boot_nvme: nvme,
        ..case(name, cmdline)
    }
}

/// R's first boot of a fresh slot disk.
const INSTALL: &[&str] = &[
    "slots: booted recovery (record missing)",
    "slots: in R with no record: installed release 1 in kai-a and kai-b as trials",
    "slots: R arms kai-a (trial)",
    "slots: next boot kai-a",
];
/// A's first boot, which commits it.
const A_COMMITS: &[&str] = &[
    "block: kai-a is ",
    "slots: booted kai-a (record read)",
    "slots: up in kai-a (trial, 2 tries left)",
    "slots: committed kai-a",
    "slots: next boot kai-a",
];
const SLOT_A: TestCase = slot(
    "slot-4k (2: kai-a commits)",
    "test=slot slot-do=commit panic=reset",
    false,
    A_COMMITS,
);
const SLOT_A_NVME: TestCase = slot(
    "slot-4k-nvme (2: kai-a commits)",
    "test=slot slot-do=commit panic=reset",
    true,
    A_COMMITS,
);

/// Part C: the whole system on disks of 4 KiB blocks. The boot disk holds
/// the slots and `kai-data` as partition 5, and the model is on a disk of
/// its own; NVMe for both when `nvme`. R installs, A commits, then a boot
/// writes files through `fs` and the next reads them back. `test=files` runs
/// no slot lever (`kernel/src/main.rs`), so those two boots print no `slots:`
/// line, and what they show is the boot disk's `kai-data` kept across boots.
const fn os(
    name: &'static str,
    cmdline: &'static str,
    nvme: bool,
    expect: &'static [&'static str],
) -> TestCase {
    TestCase {
        boot_data: true,
        disk: Some(Fixture::Gguf),
        nvme,
        resets: false,
        ..slot(name, cmdline, nvme, expect)
    }
}

const WROTE: &str =
    "filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle";
const READ_BACK: &str = "filecheck: boot 2 read back what boot 1 wrote, byte for byte";

const OS_4: TestCase = os(
    "os-4k (4: the files read back)",
    "test=files reread",
    false,
    &[MODEL, READ_BACK],
);
const OS_3: TestCase = TestCase {
    then: Some(&OS_4),
    ..os(
        "os-4k (3: files written)",
        "test=files",
        false,
        &[
            "block: kai-data is blk disk \"kai-boot\"",
            MODEL_DISK,
            MODEL,
            "fs: up; kai-data has ",
            WROTE,
        ],
    )
};
const OS_2: TestCase = TestCase {
    then: Some(&OS_3),
    resets: true,
    ..os("os-4k (2: kai-a commits)", SLOT_A.cmdline, false, A_COMMITS)
};
const OS_NVME_4: TestCase = os(
    "os-4k-nvme (4: the files read back)",
    "test=files reread",
    true,
    &[MODEL, READ_BACK],
);
const OS_NVME_3: TestCase = TestCase {
    then: Some(&OS_NVME_4),
    ..os(
        "os-4k-nvme (3: files written)",
        "test=files",
        true,
        &[
            // The NVMe serial is padded to 20 bytes; the prefix is the point.
            "block: kai-data is nvme disk \"kai-boot",
            MODEL_DISK,
            MODEL,
            "fs: up; kai-data has ",
            WROTE,
        ],
    )
};
const OS_NVME_2: TestCase = TestCase {
    then: Some(&OS_NVME_3),
    resets: true,
    ..os(
        "os-4k-nvme (2: kai-a commits)",
        SLOT_A.cmdline,
        true,
        A_COMMITS,
    )
};

/// Part C: `updated` writes slot B of a 4Kn boot disk, from the update disk
/// (`updated=update`, with a 4 KiB `kai-data` disk for `/system/trust`) or
/// as a copy of the running slot (`test=update`), in whole 4 KiB blocks
/// (`kabi::update::BLOCK`). The write reads back hash-equal and the record
/// arms B; the next boot rebuilds the slot disk, so it runs xtask's own B
/// under that record, not the bytes `updated` wrote.
const B_UP: &[&str] = &[
    "slots: booted kai-b (record read)",
    "slots: up in kai-b (trial, 2 tries left)",
    "slots: next boot kai-b",
];
const UPDATE_3: TestCase = TestCase {
    data: Some(VIRTIO),
    ..slot(
        "update-4k (3: kai-b boots release 2)",
        "test=slot panic=reset",
        false,
        B_UP,
    )
};
const UPDATE_2: TestCase = TestCase {
    then: Some(&UPDATE_3),
    data: Some(VIRTIO),
    // One sector past whole blocks: the last write is padded.
    update: Some(UpdateDisk::UNEVEN),
    ..slot(
        "update-4k (2: updated writes kai-b from the disk)",
        "test=update updated=update panic=reset",
        false,
        &[
            "updated: the update disk's repository verifies: slot.img is release 2, ",
            "updated: wrote release 2 into B and checked its sha256",
            "update: restarting into kai-b",
        ],
    )
};
const STAGE_3: TestCase = slot(
    "stage-4k-nvme (3: kai-b boots its trial)",
    "test=slot panic=reset",
    true,
    B_UP,
);
const STAGE_2: TestCase = TestCase {
    then: Some(&STAGE_3),
    import: Some(Fixture::Gguf),
    ..slot(
        "stage-4k-nvme (2: updated copies kai-a into kai-b)",
        "test=update panic=reset",
        true,
        &[
            "updated: SLOT_WRITE refuses the active slot, and R from a trial slot",
            "updated: staged B as release 2: ",
            "update: restarting into kai-b",
        ],
    )
};

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        then: Some(&OS_2),
        resets: true,
        ..os("os-4k", "test=slot panic=reset", false, INSTALL)
    },
    TestCase {
        then: Some(&OS_NVME_2),
        resets: true,
        ..os("os-4k-nvme", "test=slot panic=reset", true, INSTALL)
    },
    TestCase {
        then: Some(&UPDATE_2),
        data: Some(VIRTIO),
        ..slot("update-4k", "test=slot panic=reset", false, INSTALL)
    },
    TestCase {
        then: Some(&STAGE_2),
        ..slot("stage-4k-nvme", "test=slot panic=reset", true, INSTALL)
    },
    TestCase {
        then: Some(&SLOT_A),
        ..slot("slot-4k", "test=slot panic=reset", false, INSTALL)
    },
    TestCase {
        then: Some(&SLOT_A_NVME),
        ..slot("slot-4k-nvme", "test=slot panic=reset", true, INSTALL)
    },
    TestCase {
        then: Some(&NVME_2),
        ..four_k(
            "nvme-4k",
            "test=files",
            NVME,
            true,
            &[
                "blocks of 4096 bytes",
                "4096-byte blocks), writable",
                "block: kai-data is nvme disk ",
                MODEL,
                "fs: up; kai-data has ",
                "filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle",
                "files: boot 1: filecheck's step through its Dir handle passed",
            ],
        )
    },
    TestCase {
        then: Some(&BLK_2),
        ..four_k(
            "blk-4k",
            "test=files",
            VIRTIO,
            false,
            &[
                "4096-byte blocks), writable",
                "block: kai-data is blk disk ",
                MODEL,
                "fs: up; kai-data has ",
                "filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle",
                "files: boot 1: filecheck's step through its Dir handle passed",
            ],
        )
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blklog;
    use std::fs;

    /// A disk file as `diskmap` reads it, in sectors of `size` bytes.
    struct File(Vec<u8>, usize);

    impl diskmap::Disk for File {
        fn sectors(&self) -> u64 {
            (self.0.len() / self.1) as u64
        }
        fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
            let at = lba as usize * self.1;
            let src = self.0.get(at..at + out.len()).ok_or(diskmap::Error::Read)?;
            out.copy_from_slice(src);
            Ok(())
        }
        fn sector_size(&self) -> usize {
            self.1
        }
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kai-4k-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The 4 KiB data disk's GPT is in 4 KiB LBAs, its partitions at the
    /// same bytes as on the 512-byte disk, and it reads as no table in 512s.
    #[test]
    fn a_4k_data_disk_has_its_gpt_in_4k_lbas() {
        let (small, big) = (dir("512"), dir("4096"));
        blklog::prepare_sized(&small, VIRTIO, None, false).unwrap();
        blklog::prepare_sized(&big, VIRTIO, None, true).unwrap();
        let read = |d: &std::path::Path| fs::read(d.join(blklog::IMAGE)).unwrap();
        let (bytes_512, bytes_4k) = (read(&small), read(&big));
        assert_eq!(bytes_512.len(), bytes_4k.len());
        let t512 = diskmap::read(&mut File(bytes_512, 512)).unwrap();
        let t4k = diskmap::read(&mut File(bytes_4k.clone(), 4096)).unwrap();
        assert_eq!(t512.partitions.len(), t4k.partitions.len());
        for (a, b) in t512.partitions.iter().zip(&t4k.partitions) {
            assert_eq!((a.kind, a.first * 512), (b.kind, b.first * 4096));
            assert_eq!(a.sectors() * 512, b.sectors() * 4096, "{}", a.name);
        }
        assert!(diskmap::read(&mut File(bytes_4k, 512)).is_err());
        let args = blklog::qemu_args_in(NVME, true).join(" ");
        assert!(args.contains("nvme,") && args.contains(crate::disk::BLOCKS_4K));
        assert!(
            !blklog::qemu_args(NVME)
                .join(" ")
                .contains("logical_block_size")
        );
        fs::remove_dir_all(small).unwrap();
        fs::remove_dir_all(big).unwrap();
    }

    /// The model file is padded to whole 4 KiB blocks with zeros, while the
    /// length the guest gets stays the file's own.
    #[test]
    fn a_4k_model_disk_is_padded_and_keeps_its_length() {
        let at = dir("model");
        let plain = Fixture::Gguf.write(&at).unwrap();
        let cmdline = plain.cmdline();
        let disk = plain.on_4k().unwrap();
        let bytes = fs::read(disk.path()).unwrap();
        assert_eq!(bytes.len() % 4096, 0);
        assert_eq!(
            bytes.len() as u64,
            (crate::disk::GGUF_LEN as u64).next_multiple_of(4096)
        );
        assert!(bytes[crate::disk::GGUF_LEN..].iter().all(|&b| b == 0));
        assert_eq!(disk.cmdline(), cmdline);
        assert!(disk.qemu_args().join(" ").contains(crate::disk::BLOCKS_4K));
        fs::remove_dir_all(at).unwrap();
    }

    /// The boot disk's device for each of B's boots (G58).
    #[test]
    fn the_boot_disk_is_nvme_or_virtio_of_4k_blocks() {
        let device = |nvme, four_k| {
            let mut args = vec![
                "-drive".to_string(),
                "format=raw,file=fat:rw:esp".to_string(),
            ];
            let boot = crate::qemu::BootDisk {
                nvme,
                four_k,
                ..Default::default()
            };
            crate::qemu::boot_from_disk_on(&mut args, "slots.img", boot).unwrap();
            args[3].clone()
        };
        assert_eq!(
            device(false, false),
            "virtio-blk-pci,drive=slots,serial=kai-boot,bootindex=0"
        );
        assert_eq!(
            device(true, true),
            format!(
                "nvme,drive=slots,serial=kai-boot,bootindex=0{}",
                crate::disk::BLOCKS_4K
            )
        );
        // Every slot case boots a 4Kn slot disk on each of its boots, and a C
        // case's `kai-data` is the boot disk's on each, never a disk of its own.
        for case in CASES.iter().filter(|c| c.slots) {
            let mut boot = Some(case);
            while let Some(b) = boot {
                assert!(b.slots && b.four_k, "{}", b.name);
                assert_eq!(b.boot_data, case.boot_data, "{}", b.name);
                assert!(!b.boot_data || b.data.is_none(), "{}", b.name);
                assert!(!(b.boot_data && b.keeps_slot_b), "{}", b.name);
                boot = b.then;
            }
        }
    }
}
