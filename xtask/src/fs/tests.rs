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
    let sectors = |addr: u64| (region.offset + addr * BLOCK as u64) / 512;
    let mut entries = vec![
        Entry::Write {
            sector: 1,
            data: before[512..1024].to_vec(),
            fua: false,
        },
        Entry::Mark,
    ];
    // Runs of consecutive blocks become one write, as the driver may send them.
    for op in ops {
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
    let log = Log {
        sector_size: 512,
        entries,
    };
    (before, region, log)
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
