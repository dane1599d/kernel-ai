//! The model disk: a raw file attached read-only as a virtio-blk device with the
//! serial `kai-model`, and the fixture files the boot cases attach.
//!
//! QEMU runs in `target/`, and the other drives use relative `-drive file=` paths,
//! whose values QEMU parses for protocol prefixes, colons and commas. A model file
//! lives on another drive (`D:\kai-models` by default), so its path cannot be
//! relative. It goes in a `-blockdev` written as JSON instead: the path is one JSON
//! string handed to QEMU's `file` driver as it is, drive letter, spaces, commas and
//! all. Nothing is copied and no junction is made. On Windows the path must be
//! ASCII, since QEMU there takes its arguments in the ANSI code page.

use kabi::model::{Attached, Fnv1a, SERIAL};
use std::fs;
use std::path::{Path, PathBuf};

/// Guest memory without a model disk, in MiB.
pub const BASE_MEMORY_MIB: u64 = 512;
const MIB: u64 = 1024 * 1024;
/// The model's share of guest memory is rounded up to a multiple of this.
const MEMORY_STEP_MIB: u64 = 64;
const RAW_LEN: usize = 3 * MIB as usize + 777;
const GGUF_LEN: usize = 2 * MIB as usize + 4093;

pub struct ModelDisk {
    path: PathBuf,
    /// The path as QEMU gets it.
    filename: String,
    len: u64,
    /// The host's hash, passed on for a test boot to check against.
    fnv: Option<u64>,
    /// The answer the model must give to `ask hi`, for the `local-ask` case.
    golden: Option<String>,
}

impl ModelDisk {
    /// A file to attach, by an absolute path. The file is only read.
    pub fn open(path: &Path) -> Result<Self, String> {
        // Not canonicalize: on Windows that gives a `\\?\` path QEMU may not take.
        let path = std::path::absolute(path)
            .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
        let meta = fs::metadata(&path)
            .map_err(|e| format!("cannot read the model disk {}: {e}", path.display()))?;
        if !meta.is_file() || meta.len() == 0 {
            return Err(format!(
                "the model disk {} is not a non-empty file",
                path.display()
            ));
        }
        let filename = path
            .to_str()
            .ok_or_else(|| format!("the model disk path {} is not UTF-8", path.display()))?
            .to_string();
        // QEMU for Windows reads its arguments and opens files in the ANSI code
        // page, so only an ASCII path reaches the file intact.
        if cfg!(windows) && !filename.is_ascii() {
            return Err(format!(
                "QEMU on Windows cannot open {filename}: move or rename it to an ASCII path"
            ));
        }
        Ok(Self {
            path,
            filename,
            len: meta.len(),
            fnv: None,
            golden: None,
        })
    }

    /// What `run` prints about the disk.
    pub fn describe(&self) -> String {
        format!(
            "{} attached read-only as the kai-model disk ({} bytes), guest memory {} MiB",
            self.path.display(),
            self.len,
            self.memory_mib()
        )
    }

    /// Guest memory for a VM with this disk: the usual amount, room for the copy
    /// the kernel keeps, and twice the pages llm's engine gets with a model, since
    /// processes may hold half of what is free; so every other service gets as
    /// much as without it.
    pub fn memory_mib(&self) -> u64 {
        let engine_mib = kabi::model::ENGINE_PAGES * 4096 / MIB;
        let extra = self.len.div_ceil(MIB) + 2 * engine_mib;
        BASE_MEMORY_MIB + extra.next_multiple_of(MEMORY_STEP_MIB)
    }

    /// The QEMU arguments that attach the file read-only.
    pub fn qemu_args(&self) -> Vec<String> {
        let blockdev = serde_json::json!({
            "node-name": SERIAL,
            "driver": "raw",
            "read-only": true,
            "file": {
                "driver": "file",
                "filename": self.filename,
                "read-only": true,
            },
        });
        vec![
            "-blockdev".into(),
            blockdev.to_string(),
            "-device".into(),
            format!("virtio-blk-pci,drive={SERIAL},serial={SERIAL}"),
        ]
    }

    /// The kernel command-line words that describe the file, and the hash of the
    /// golden answer when there is one.
    pub fn cmdline(&self) -> String {
        let attached = Attached {
            len: Some(self.len),
            fnv: self.fnv,
        }
        .to_cmdline();
        match &self.golden {
            Some(golden) => format!(
                "{attached} local_golden={:016x}",
                kabi::model::fnv1a(golden.as_bytes())
            ),
            None => attached,
        }
    }

    /// Console lines a boot with this disk must show exactly: the terminal's
    /// answer to `ask hi`.
    pub fn exact_lines(&self) -> Vec<String> {
        self.golden
            .as_deref()
            .map_or_else(Vec::new, crate::local_ask::terminal_lines)
    }

    /// Text such a boot's console must contain: health naming the local model.
    pub fn console_text(&self) -> Vec<String> {
        match self.golden {
            // After the model's length and hash: llm's start-up line has no `; `.
            Some(_) => vec![format!(
                "; local model {} (",
                infer::fixture::LOCAL_ASK_NAME
            )],
            None => Vec::new(),
        }
    }
}

/// The disk a test case attaches, written under `target/fixtures/`, if it has one.
pub fn for_case(fixture: Option<Fixture>, target_dir: &Path) -> Result<Option<ModelDisk>, String> {
    fixture
        .map(|fixture| fixture.write(&target_dir.join("fixtures")))
        .transpose()
}

/// The deterministic, odd-sized files the boot cases attach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    /// Bytes without the GGUF magic, read raw by the `blk` case, which the kernel
    /// must not take for a model.
    Raw,
    /// The GGUF v3 magic and version, then filler: the kernel checks only those
    /// eight bytes, so the `model` case can map it into llm, which cannot run it.
    Gguf,
    /// The generated model `local-ask` answers from (`infer::fixture::local_ask`),
    /// with the golden answer xtask computes for it.
    LocalAsk,
}

impl Fixture {
    fn file_name(self) -> &'static str {
        match self {
            Fixture::Raw => "blk-raw.img",
            Fixture::Gguf => "model.gguf",
            Fixture::LocalAsk => "local-ask.gguf",
        }
    }

    /// Every fixture is an odd number of bytes, not whole 512-byte sectors; the
    /// filler ones are a few MiB.
    fn bytes(self) -> Vec<u8> {
        let (lead, seed, len): (&[u8], u64, usize) = match self {
            Fixture::Raw => (
                b"Kernel AI blk fixture, not a model\n",
                0x6b61_692d_626c_6b00,
                RAW_LEN,
            ),
            Fixture::Gguf => (
                b"GGUF\x03\0\0\0Kernel AI model fixture\n",
                0x6b61_692d_6767_7566,
                GGUF_LEN,
            ),
            Fixture::LocalAsk => return infer::fixture::local_ask(),
        };
        let mut bytes = Vec::with_capacity(len);
        bytes.extend_from_slice(lead);
        let mut state = seed;
        while bytes.len() < len {
            // xorshift64: cheap, fixed, and every byte value appears.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.extend_from_slice(&state.to_le_bytes());
        }
        bytes.truncate(len);
        bytes
    }

    /// Writes the fixture under `dir` and opens it, with its hash for the guest to
    /// check.
    pub fn write(self, dir: &Path) -> Result<ModelDisk, String> {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let path = dir.join(self.file_name());
        let bytes = self.bytes();
        fs::write(&path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let mut hash = Fnv1a::new();
        hash.update(&bytes);
        let mut disk = ModelDisk::open(&path)?;
        disk.fnv = Some(hash.finish());
        if self == Fixture::LocalAsk {
            disk.golden = Some(crate::local_ask::golden(&bytes)?);
        }
        Ok(disk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_disk(path: &str, len: u64) -> ModelDisk {
        ModelDisk {
            path: PathBuf::from(path),
            filename: path.to_string(),
            len,
            fnv: None,
            golden: None,
        }
    }

    #[test]
    fn a_windows_model_path_reaches_qemu_whole() {
        let path = r"D:\kai-models\Qwen3 0.6B,Q8_0\qwen3-0.6b-q8_0.gguf";
        let args = windows_disk(path, 639_446_688).qemu_args();
        assert_eq!(args[0], "-blockdev");
        // The value is JSON, so QEMU never splits it at a colon or a comma.
        assert!(args[1].starts_with('{'));
        let spec: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(spec["node-name"], SERIAL);
        assert_eq!(spec["driver"], "raw");
        assert_eq!(spec["read-only"], true);
        assert_eq!(spec["file"]["driver"], "file");
        assert_eq!(spec["file"]["filename"], path);
        assert_eq!(spec["file"]["read-only"], true);
        assert_eq!(args[2], "-device");
        assert_eq!(args[3], "virtio-blk-pci,drive=kai-model,serial=kai-model");
    }

    #[test]
    fn memory_grows_with_the_model_and_the_engine() {
        // The engine's 136 MiB, twice: 272 MiB on top of the model.
        assert_eq!(kabi::model::ENGINE_PAGES * 4096 / MIB, 136);
        assert_eq!(
            windows_disk("D:/m.gguf", 639_446_688).memory_mib(),
            512 + 896
        );
        assert_eq!(
            windows_disk("D:/m.gguf", 3 * MIB + 777).memory_mib(),
            512 + 320
        );
        assert_eq!(windows_disk("D:/m.gguf", 1).memory_mib(), 512 + 320);
    }

    #[test]
    fn the_command_line_names_the_length_and_a_test_hash() {
        let mut disk = windows_disk("D:/m.gguf", 3_146_505);
        assert_eq!(disk.cmdline(), "model_len=3146505");
        disk.fnv = Some(0xfeed);
        assert_eq!(
            disk.cmdline(),
            "model_len=3146505 model_fnv=000000000000feed"
        );
    }

    #[test]
    fn fixtures_are_odd_sized_stable_and_not_gguf() {
        let bytes = Fixture::Raw.bytes();
        assert_eq!(bytes.len(), RAW_LEN);
        assert_ne!(bytes.len() % 512, 0);
        assert!(!bytes.starts_with(b"GGUF"));
        assert_eq!(
            kabi::model::fnv1a(&bytes),
            kabi::model::fnv1a(&Fixture::Raw.bytes())
        );
    }

    #[test]
    fn the_model_fixture_passes_only_the_kernel_check() {
        let bytes = Fixture::Gguf.bytes();
        assert_eq!(bytes.len(), GGUF_LEN);
        assert_ne!(bytes.len() % 512, 0);
        assert_eq!(kabi::model::check_header(&bytes), Ok(()));
        assert_eq!(
            kabi::model::check_header(&Fixture::Raw.bytes()),
            Err(kabi::model::HeaderError::NoMagic)
        );
    }

    #[test]
    fn the_local_ask_disk_carries_its_golden_answer() {
        let dir = std::env::temp_dir().join(format!("kai-local-ask-{}", std::process::id()));
        let disk = Fixture::LocalAsk.write(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(disk.len % 2, 1);
        assert_ne!(disk.len % 512, 0);
        let golden = crate::local_ask::golden(&infer::fixture::local_ask()).unwrap();
        let hash = kabi::model::fnv1a(golden.as_bytes());
        assert!(
            disk.cmdline()
                .ends_with(&format!(" local_golden={hash:016x}")),
            "{}",
            disk.cmdline()
        );
        assert!(
            disk.cmdline()
                .starts_with(&format!("model_len={}", disk.len))
        );
        assert_eq!(disk.exact_lines(), [format!("= {golden}")]);
        assert_eq!(disk.console_text(), ["; local model kai test model ("]);
        // The other fixtures have no answer to check.
        let raw = windows_disk("D:/m.gguf", 5);
        assert!(raw.exact_lines().is_empty() && raw.console_text().is_empty());
    }

    #[test]
    fn open_refuses_a_missing_file() {
        assert!(ModelDisk::open(Path::new("no/such/model.gguf")).is_err());
    }
}
