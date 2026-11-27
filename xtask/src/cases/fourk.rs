//! Disks of 4 KiB blocks (G58, part A): the data disk and the model disk on
//! NVMe and on virtio-blk with `logical_block_size=4096`. `kai-data` on kaifs
//! takes files through `fs` on boot 1 and reads them back on boot 2, and the
//! model is read whole from its own disk on both boots. The boot disk keeps
//! 512-byte sectors here; parts B and C move it.

use super::{TestCase, case};
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

pub(super) const CASES: &[TestCase] = &[
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
}
