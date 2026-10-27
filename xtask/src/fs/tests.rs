//! `cargo xtask fs` on the host (P2 task 7): the data disk, each command, the
//! log reader on hostile input (diskmap tests its own GPT reader), and the
//! blklogwrites replay of a real kaifs run. Every path here has a space in it,
//! as the founder's checkout does.

#[path = "../../../testkit/hostile.rs"]
mod hostile;

use super::blklog::{self, Entry, Log};
use super::disk::{self, Bytes, FileDev, Region};
use super::ops;
use kaifs::fs::{Faults, HOME, MODELS, SYSTEM};
use kaifs::{BLOCK, Fs, Key, MemDisk, Op, ROOT, RecordingDisk};
use std::path::{Path, PathBuf};

/// A fresh folder for one test, under the workspace's `target/`.
fn scratch(name: &str) -> PathBuf {
    let dir = crate::workspace_root()
        .join("target")
        .join("xtask fs tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn made(dir: &Path) -> PathBuf {
    let image = dir.join("kai data.img");
    ops::mkfs(&image, 24 << 20, "test", false).unwrap();
    image
}

fn cat(image: &Path, path: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ops::cat(image, path, &mut out).unwrap();
    out
}

#[test]
fn guest_paths_name_their_volume() {
    assert_eq!(ops::split("/home").unwrap(), (HOME, "/".into()));
    assert_eq!(
        ops::split("/home/user/a.txt").unwrap(),
        (HOME, "/user/a.txt".into())
    );
    assert_eq!(
        ops::split("/system/settings/").unwrap(),
        (SYSTEM, "/settings".into())
    );
    assert_eq!(
        ops::split("/system/models/x.gguf").unwrap(),
        (MODELS, "/x.gguf".into())
    );
    assert_eq!(
        ops::split("/system/modelsx").unwrap(),
        (SYSTEM, "/modelsx".into())
    );
    for bad in ["home", "/homes", "/", "/tmp/x", ""] {
        assert!(ops::split(bad).is_err(), "{bad}");
    }
}

#[test]
fn mkfs_makes_a_gpt_disk_whose_kai_data_partition_holds_kaifs() {
    let dir = scratch("mkfs");
    let image = made(&dir);
    assert!(
        ops::mkfs(&image, 24 << 20, "test", false).is_err(),
        "it overwrote an image"
    );
    assert!(ops::mkfs(&image, 24 << 20, "test", true).is_ok());
    assert!(ops::mkfs(&dir.join("small.img"), 8 << 20, "test", false).is_err());
    let bytes = std::fs::read(&image).unwrap();
    assert_eq!(bytes.len(), 24 << 20);
    assert_eq!(&bytes[512..520], b"EFI PART");
    let table = diskmap::read(&mut Bytes(&bytes)).unwrap();
    assert!(
        table.find(diskmap::KAI_DATA_TYPE).is_some(),
        "diskmap finds kai-data"
    );
    let region = disk::locate(&mut Bytes(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(region.offset, 2 << 20);
    assert!(region.blocks * BLOCK as u64 + region.offset < bytes.len() as u64);
    // Block 0 of kai-data stays outside kaifs, for kai-modelhint 1.
    let at = region.offset as usize;
    assert!(bytes[at..at + BLOCK].iter().all(|&b| b == 0));
    assert_eq!(
        ops::ls(&image, "/").unwrap(),
        ["home/", "system/", "system/models/"]
    );
    let home: Vec<String> = ops::ls(&image, "/home").unwrap();
    assert!(
        home.iter().any(|l| l.ends_with(" .trash/")) && home.iter().any(|l| l.ends_with(" user/"))
    );
    let system = ops::ls(&image, "/system").unwrap();
    for folder in ["pkg/", "settings/", "state/"] {
        assert!(system.iter().any(|l| l.ends_with(folder)), "{system:?}");
    }
    let lines = ops::check_image(&image, false).unwrap();
    assert!(lines[0].ends_with("checks"), "{lines:?}");
}

#[test]
fn import_copies_a_folder_tree_and_cat_reads_it_back() {
    let dir = scratch("import");
    let image = made(&dir);
    let host = dir.join("host files");
    std::fs::create_dir_all(host.join("deep folder").join("deeper")).unwrap();
    let big: Vec<u8> = (0..2_500_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(host.join("big.bin"), &big).unwrap();
    std::fs::write(
        host.join("\u{65e5}\u{8a18}.txt"),
        "kaifs \u{306e}\u{65e5}\u{8a18}",
    )
    .unwrap();
    std::fs::write(host.join("empty"), b"").unwrap();
    std::fs::write(
        host.join("deep folder").join("deeper").join("note.md"),
        b"# note\n",
    )
    .unwrap();
    let done = ops::import(&image, &host, "/home/user/imported").unwrap();
    assert!(done.contains("4 files"), "{done}");
    assert_eq!(cat(&image, "/home/user/imported/big.bin"), big);
    assert_eq!(
        cat(&image, "/home/user/imported/\u{65e5}\u{8a18}.txt"),
        "kaifs \u{306e}\u{65e5}\u{8a18}".as_bytes()
    );
    assert_eq!(cat(&image, "/home/user/imported/empty"), b"");
    assert_eq!(
        cat(&image, "/home/user/imported/deep folder/deeper/note.md"),
        b"# note\n"
    );
    let listed = ops::ls(&image, "/home/user/imported").unwrap();
    assert_eq!(listed.len(), 4);
    assert!(
        listed
            .iter()
            .any(|l| l.trim_start().starts_with("2500000  big.bin")),
        "{listed:?}"
    );
    // A second import replaces what is there; a single file goes where it is
    // told.
    std::fs::write(host.join("empty"), b"no longer").unwrap();
    ops::import(&image, &host, "/home/user/imported").unwrap();
    assert_eq!(cat(&image, "/home/user/imported/empty"), b"no longer");
    ops::import(&image, &host.join("big.bin"), "/system/settings/big.cbor").unwrap();
    assert_eq!(cat(&image, "/system/settings/big.cbor"), big);
    assert!(ops::import(&image, &host, "/home/none/x").is_err());
    assert!(ops::import(&image, &host, "/home").is_err());
    assert!(ops::cat(&image, "/home/user/imported", &mut Vec::new()).is_err());
    ops::check_image(&image, false).unwrap();
}

#[test]
fn a_raw_image_without_a_gpt_is_kaifs_over_the_whole_file() {
    let dir = scratch("raw");
    let image = dir.join("raw.img");
    let file = std::fs::File::create(&image).unwrap();
    file.set_len(16 << 20).unwrap();
    let dev = FileDev::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&image)
            .unwrap(),
        Region {
            offset: 0,
            blocks: 4096,
        },
    );
    let mut fs = Fs::mkfs(dev, [1; 16], "raw").unwrap();
    fs.set_volume(HOME).unwrap();
    let f = fs.create(ROOT, "x").unwrap();
    fs.write_file(f, b"raw").unwrap();
    fs.commit().unwrap();
    drop(fs);
    assert_eq!(cat(&image, "/home/x"), b"raw");
    ops::check_image(&image, false).unwrap();
}

#[test]
fn check_repairs_what_it_finds_wrong() {
    let dir = scratch("repair");
    let image = made(&dir);
    {
        let mut fs = ops::open(&image, true).unwrap();
        fs.set_volume(HOME).unwrap();
        fs.put(Key::new(ROOT, 9, 0), b"an item of no kind format 1 has")
            .unwrap();
        fs.commit().unwrap();
    }
    let found = ops::check_image(&image, false).unwrap_err();
    assert!(found.contains("does not check"), "{found}");
    let lines = ops::check_image(&image, true).unwrap();
    assert!(
        lines.iter().any(|l| l.contains("home: object 1: dropped")),
        "{lines:?}"
    );
    assert!(lines.last().unwrap().contains("held by nothing"));
    ops::check_image(&image, false).unwrap();
}

/// A disk no commit of which mounts, its allocator's chain overwritten:
/// `check --repair` mounts it from what the trees reach, and it checks after.
#[test]
fn check_repair_mounts_a_disk_whose_allocator_does_not_read() {
    let dir = scratch("allocator");
    let image = made(&dir);
    let chain: Vec<u64> = {
        let fs = ops::open(&image, false).unwrap();
        fs.io.alloc.chain().iter().map(|p| p.addr).collect()
    };
    let mut bytes = std::fs::read(&image).unwrap();
    let region = disk::locate(&mut Bytes(&bytes), bytes.len() as u64).unwrap();
    for addr in chain {
        let at = (region.offset + addr * BLOCK as u64) as usize;
        bytes[at..at + BLOCK].fill(0xA5);
    }
    std::fs::write(&image, &bytes).unwrap();
    assert!(ops::check_image(&image, false).is_err());
    let lines = ops::check_image(&image, true).unwrap();
    assert!(lines[0].contains("the disk does not mount"), "{lines:?}");
    ops::check_image(&image, false).unwrap();
    let home = ops::ls(&image, "/home").unwrap();
    assert!(home.iter().any(|line| line.contains("user")), "{home:?}");
}

/// `import --model`: the blob lands sealed under its SHA-256, its extents
/// hold it as `MODEL_FILL` will read them, and sector 0 names it.
#[test]
fn import_model_writes_a_sealed_blob_and_names_it_in_sector_0() {
    use kabi::modelhint::Hint;
    use sha2::{Digest, Sha256};
    let dir = scratch("model");
    let image = made(&dir);
    let model = dir.join("tiny.gguf");
    let mut bytes = b"GGUF".to_vec();
    bytes.extend((0..(5u32 << 20) + 77).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8));
    std::fs::write(&model, &bytes).unwrap();
    let line = super::model::import_model(&image, &model).unwrap();
    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
    let name: String = sha256.iter().map(|b| format!("{b:02x}")).collect();
    let path = format!("/system/models/{name}.gguf");
    assert!(line.starts_with(&path), "{line}");
    assert_eq!(cat(&image, &path), bytes);
    let fnv = kabi::model::fnv1a(&bytes);
    let disk = std::fs::read(&image).unwrap();
    let region = disk::locate(&mut Bytes(&disk), disk.len() as u64).unwrap();
    let at = region.offset as usize;
    let hint = Hint::decode(&disk[at..at + 512]).unwrap();
    let len = bytes.len() as u64;
    assert_eq!(hint, Hint { len, fnv, sha256 });
    let held = {
        let mut fs = ops::open(&image, false).unwrap();
        fs.set_volume(MODELS).unwrap();
        let ino = fs.resolve(&format!("/{name}.gguf")).unwrap();
        assert!(fs.is_sealed(ino).unwrap());
        let attr = fs.get_xattr(ino, super::model::FNV_ATTR).unwrap();
        assert_eq!(attr.unwrap(), fnv.to_le_bytes());
        let mut held = Vec::new();
        for (start, blocks) in fs.sealed_extents(ino).unwrap() {
            let from = at + (start * BLOCK as u64) as usize;
            held.extend_from_slice(&disk[from..from + (blocks * BLOCK as u64) as usize]);
        }
        held
    };
    assert_eq!(held[..bytes.len()], bytes[..]);
    ops::check_image(&image, false).unwrap();
    // Again: named active, not written twice.
    let again = super::model::import_model(&image, &model).unwrap();
    assert!(again.contains("was there already"), "{again}");
    let after = ops::ls(&image, "/system/models").unwrap();
    assert_eq!(
        after.iter().filter(|l| l.contains(".gguf")).count(),
        1,
        "{after:?}"
    );
    // Not a GGUF file: refused, and nothing written.
    std::fs::write(&model, b"not a model").unwrap();
    assert!(super::model::import_model(&image, &model).is_err());
    ops::check_image(&image, false).unwrap();
}

/// kaifs on a case disk as task 2's `make_disk` lays it out, `kai-cache` after
/// `kai-data`: every command finds `kai-data` by its type GUID and leaves the
/// cache alone. A disk whose primary GPT is gone is read from its backup.
#[test]
fn a_case_disk_with_kai_cache_is_found_by_type_guid() {
    let dir = scratch("case disk");
    let image = dir.join("case disk.img");
    crate::blklog::make_disk(&image, Some(1)).unwrap();
    let mut fs = Fs::mkfs(FileDev::open(&image, true).unwrap(), [2; 16], "case").unwrap();
    fs.set_volume(HOME).unwrap();
    let f = fs.create(ROOT, "note").unwrap();
    fs.write_file(f, b"on a case disk").unwrap();
    fs.commit().unwrap();
    drop(fs);
    assert_eq!(cat(&image, "/home/note"), b"on a case disk");
    let bytes = std::fs::read(&image).unwrap();
    let region = disk::locate(&mut Bytes(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(
        region,
        Region {
            offset: crate::blklog::DATA_FIRST * 512,
            blocks: crate::blklog::DATA_SECTORS / 8
        }
    );
    let cache_at = ((crate::blklog::DATA_FIRST + crate::blklog::DATA_SECTORS) * 512) as usize;
    assert!(
        bytes[cache_at..cache_at + (1 << 20)]
            .iter()
            .all(|&b| b == 0)
    );
    // The primary header gone: diskmap reads the backup, and kaifs is found.
    let mut broken = bytes.clone();
    broken[512..1024].fill(0);
    assert_eq!(
        disk::locate(&mut Bytes(&broken), broken.len() as u64).unwrap(),
        region
    );
    ops::check_image(&image, false).unwrap();
}

/// A kaifs run on a recording disk over `region` of a fresh image, as a
/// blklogwrites log would record it: writes of runs of blocks, flushes, a write
/// to the GPT and a mark. Returns the base image, the region and the log.
fn recorded_run(name: &str, faults: Faults) -> (Vec<u8>, Region, Log) {
    let dir = scratch(name);
    let image = made(&dir);
    let before = std::fs::read(&image).unwrap();
    let region = disk::locate(&mut Bytes(&before), before.len() as u64).unwrap();
    let mut rec = RecordingDisk::new(region.blocks);
    rec.disk = blklog::base(&before, region);
    let mut fs = Fs::mount_with(rec, faults).unwrap();
    for round in 0..4u64 {
        fs.begin(round + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][round as usize % 2]).unwrap();
        let f = fs.create(ROOT, &format!("file {round}")).unwrap();
        fs.write_file(f, &vec![round as u8 + 1; 9000 + round as usize * 4096])
            .unwrap();
        fs.commit().unwrap();
    }
    let ops = std::mem::take(&mut fs.io.dev.log);
    let log = log_of(&ops, region, &before[512..1024]);
    (before, region, log)
}

/// `ops` on `region` as a blklogwrites log would record them, after a write of
/// `gpt` to sector 1 and a mark: runs of consecutive blocks become one write, as
/// the driver may send them.
fn log_of(ops: &[Op], region: Region, gpt: &[u8]) -> Log {
    let sectors = |addr: u64| (region.offset + addr * BLOCK as u64) / 512;
    let mut entries = vec![
        Entry::Write {
            sector: 1,
            data: gpt.to_vec(),
            fua: false,
        },
        Entry::Mark,
    ];
    for op in ops.iter().cloned() {
        match (op, entries.last_mut()) {
            (Op::Flush, _) => entries.push(Entry::Flush),
            (Op::Write(addr, block), Some(Entry::Write { sector, data, .. }))
                if *sector >= sectors(0)
                    && *sector + (data.len() / 512) as u64 == sectors(addr) =>
            {
                data.extend_from_slice(&block[..]);
            }
            (Op::Write(addr, block), _) => entries.push(Entry::Write {
                sector: sectors(addr),
                data: block.to_vec(),
                fua: false,
            }),
        }
    }
    Log {
        sector_size: 512,
        entries,
    }
}

#[test]
fn a_replayed_log_of_a_kaifs_run_checks_in_every_crash_state() {
    let (before, region, log) = recorded_run("replay", Faults::default());
    let bytes = blklog::encode(&log);
    assert_eq!(blklog::parse(&bytes).unwrap(), log);
    assert!(
        log.entries
            .iter()
            .any(|e| matches!(e, Entry::Write { data, .. } if data.len() > BLOCK))
    );
    let tally = blklog::replay(&blklog::base(&before, region), &log, region).unwrap();
    assert_eq!(tally.violations, 0, "{:#?}", tally.examples);
    assert!(tally.states > 500, "only {} states", tally.states);
}

#[test]
fn a_replayed_log_finds_a_missing_flush() {
    let faults = Faults {
        skip_data_flush: true,
        ..Faults::default()
    };
    let (before, region, log) = recorded_run("replay without a flush", faults);
    let tally = blklog::replay(&blklog::base(&before, region), &log, region).unwrap();
    assert!(
        tally.violations > 0,
        "a commit without its data flush went unnoticed"
    );
}

#[test]
fn hostile_logs_are_read_or_refused() {
    let (_, region, log) = recorded_run("hostile log", Faults::default());
    let bytes = blklog::encode(&log);
    let sample = &bytes[..bytes.len().min(12_000)];
    hostile::fuzz(sample, 23, 3_000, |b| {
        if let Ok(parsed) = blklog::parse(b) {
            let _ = blklog::ops(&parsed, region);
        }
    });
    hostile::sweep_fields(&bytes[..2048], 0..2048, |_, b| {
        let _ = blklog::parse(b);
    });
    // A write that covers part of a kai-data block, or a discard there, is
    // refused: kaifs sends neither.
    for entry in [
        Entry::Write {
            sector: region.offset / 512 + 1,
            data: vec![0; 512],
            fua: false,
        },
        Entry::Discard {
            sector: region.offset / 512,
            sectors: 8,
        },
    ] {
        let log = Log {
            sector_size: 512,
            entries: vec![entry],
        };
        assert!(blklog::ops(&log, region).is_err());
    }
    let empty = Log {
        sector_size: 512,
        entries: Vec::new(),
    };
    let tally = blklog::replay(
        &MemDisk::new(16),
        &empty,
        Region {
            offset: 0,
            blocks: 16,
        },
    )
    .unwrap();
    assert_eq!(
        tally.states, 1,
        "the one state, the base, which mounts nothing"
    );
}

#[test]
fn the_command_line_takes_each_command_and_refuses_what_it_does_not_know() {
    let dir = scratch("command line");
    let image = dir.join("cli disk.img");
    let at = image.to_str().unwrap().to_string();
    let run = |args: &[&str]| super::main(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
    run(&["mkfs", &at, "--size", "24", "--label", "cli"]).unwrap();
    assert!(run(&["mkfs", &at, "--size", "24"]).is_err());
    run(&["mkfs", &at, "--size", "24", "--force"]).unwrap();
    std::fs::write(dir.join("one.txt"), b"one").unwrap();
    let one = dir.join("one.txt");
    run(&["import", &at, one.to_str().unwrap(), "/home/user/one.txt"]).unwrap();
    run(&["ls", &at]).unwrap();
    run(&["ls", &at, "/home/user"]).unwrap();
    run(&["check", &at]).unwrap();
    run(&["check", &at, "--repair"]).unwrap();
    for bad in [
        &["mkfs"][..],
        &["mkfs", &at, "--size"],
        &["mkfs", &at, "--size", "big"],
        &["mkfs", &at, "--speed", "9"],
        &["ls"],
        &["cat", &at],
        &["unknown", &at],
        &["replay", &at],
    ] {
        assert!(run(bad).is_err(), "{bad:?}");
    }
    assert!(super::main(&[]).is_err());
    // replay reads its two files: a clean log passes, one that loses the data
    // flush fails.
    for (name, faults, passes) in [
        ("cli replay", Faults::default(), true),
        (
            "cli replay without a flush",
            Faults {
                skip_data_flush: true,
                ..Faults::default()
            },
            false,
        ),
    ] {
        let (before, _, log) = recorded_run(name, faults);
        let (base, log_file) = (
            dir.join(format!("{name} base.img")),
            dir.join(format!("{name}.log")),
        );
        std::fs::write(&base, &before).unwrap();
        std::fs::write(&log_file, blklog::encode(&log)).unwrap();
        let replayed = run(&["replay", base.to_str().unwrap(), log_file.to_str().unwrap()]);
        assert_eq!(replayed.is_ok(), passes, "{name}: {replayed:?}");
    }
}

/// A case's data disk after a run, in `target/` as the case leaves it: task 2's
/// disk with `kai-cache`, kaifs in `kai-data` (made on the host first if
/// `host_mkfs`, else by the run itself), `actions` file actions, and the log.
/// Returns the base the case would have saved, for a host-made disk.
fn case_disk(target: &Path, actions: u64, host_mkfs: bool, faults: Faults) -> Option<PathBuf> {
    let image = target.join(crate::blklog::IMAGE);
    crate::blklog::make_disk(&image, Some(1)).unwrap();
    if host_mkfs {
        let dev = FileDev::open(&image, true).unwrap();
        Fs::mkfs(dev, [6; 16], "case").unwrap();
    }
    let before = std::fs::read(&image).unwrap();
    let region = disk::locate(&mut Bytes(&before), before.len() as u64).unwrap();
    let mut rec = RecordingDisk::new(region.blocks);
    rec.disk = blklog::base(&before, region);
    let mut fs = match host_mkfs {
        true => Fs::mount_with(rec, faults).unwrap(),
        false => Fs::mkfs_with(rec, [7; 16], "guest", faults).unwrap(),
    };
    for n in 0..actions {
        fs.begin(n + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][n as usize % 2]).unwrap();
        let name = format!("action {}", n % 25);
        let f = match fs.lookup(ROOT, &name).unwrap() {
            Some(e) => e.ino,
            None => fs.create(ROOT, &name).unwrap(),
        };
        fs.write_file(f, &vec![n as u8; 600 + (n as usize * 997) % 9000])
            .unwrap();
        fs.commit().unwrap();
    }
    // What QEMU leaves: the disk with every write on it, and the log.
    let ops = std::mem::take(&mut fs.io.dev.log);
    let mut after = before.clone();
    for op in &ops {
        if let Op::Write(addr, block) = op {
            let at = (region.offset + addr * BLOCK as u64) as usize;
            after[at..at + BLOCK].copy_from_slice(&block[..]);
        }
    }
    std::fs::write(&image, &after).unwrap();
    let log = log_of(&ops, region, &before[512..1024]);
    std::fs::write(target.join(crate::blklog::LOG), blklog::encode(&log)).unwrap();
    host_mkfs.then(|| {
        let base = target.join("kai-data.base.img");
        std::fs::write(&base, &before).unwrap();
        base
    })
}

#[test]
fn a_case_disk_the_guest_formatted_replays_clean() {
    use crate::blklog::Expect;
    let target = scratch("case guest mkfs");
    case_disk(&target, 12, false, Faults::default());
    let figures = blklog::check_case(&target, None, Expect::Clean).unwrap();
    assert!(figures.contains(" 0 violations"), "{figures}");
    // The deliberate bug is no bug here, so Caught must fail.
    assert!(blklog::check_case(&target, None, Expect::Caught).is_err());
}

#[test]
fn a_case_disk_formatted_on_the_host_needs_its_base() {
    use crate::blklog::Expect;
    let target = scratch("case host mkfs");
    let base = case_disk(&target, 8, true, Faults::default()).unwrap();
    blklog::check_case(&target, Some(&base), Expect::Clean).unwrap();
    let without = blklog::check_case(&target, None, Expect::Clean).unwrap_err();
    assert!(without.contains("not the disk before the run"), "{without}");
    // The same from the command line.
    let run = |args: &[&str]| super::main(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
    let (target, base) = (target.to_str().unwrap(), base.to_str().unwrap());
    run(&["case", target, "--base", base]).unwrap();
    assert!(run(&["case", target, "--base", base, "--caught"]).is_err());
    assert!(run(&["case", target]).is_err());
}

#[test]
fn a_cut_log_is_refused_before_any_state_is_judged() {
    use crate::blklog::Expect;
    let target = scratch("case cut log");
    case_disk(&target, 6, false, Faults::default());
    let path = target.join(crate::blklog::LOG);
    let mut log = blklog::parse(&std::fs::read(&path).unwrap()).unwrap();
    log.entries.truncate(log.entries.len() - 3);
    std::fs::write(&path, blklog::encode(&log)).unwrap();
    let cut = blklog::check_case(&target, None, Expect::Clean).unwrap_err();
    assert!(cut.contains("cut short"), "{cut}");
}

#[test]
fn a_case_run_without_its_data_flush_is_caught() {
    use crate::blklog::Expect;
    let target = scratch("case no flush");
    let faults = Faults {
        skip_data_flush: true,
        ..Faults::default()
    };
    case_disk(&target, 6, false, faults);
    blklog::check_case(&target, None, Expect::Caught).unwrap();
    assert!(blklog::check_case(&target, None, Expect::Clean).is_err());
}

/// Task 9's size: 200 file actions on the case disk. The replay's time is the
/// figure; run it with `cargo test -p xtask --release -- --ignored two_hundred`.
#[test]
#[ignore = "task 9's full size: a release build takes seconds, a debug one minutes"]
fn two_hundred_actions_replay_in_a_ci_minute() {
    use crate::blklog::Expect;
    let target = scratch("case 200 actions");
    case_disk(&target, 200, false, Faults::default());
    let start = std::time::Instant::now();
    let figures = blklog::check_case(&target, None, Expect::Clean).unwrap();
    eprintln!("{figures}");
    assert!(start.elapsed().as_secs() < 60, "{figures}");
}
