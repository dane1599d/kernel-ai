//! `check --repair` on whole filesystems damaged at random (kaifs hardening):
//! blocks of a filesystem with history in two volumes overwritten with garbage,
//! zeros, a flipped bit or another block. Each case must be refused by the
//! repair mount, or repaired to a filesystem that mounts and checks; a second
//! repair then finds nothing, and a filesystem that checked is left alone.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::XorShift;
use kaifs::check::{Cache, check};
use kaifs::fs::{HOME, SYSTEM};
use kaifs::ns_check::check_namespace;
use kaifs::repair_fs::{mount_to_repair, repair_fs};
use kaifs::{BLOCK, Block, Fs, MemDisk, ROOT, format};
use std::rc::Rc;

const BLOCKS: u64 = 4096;

fn checks(fs: &mut Fs<MemDisk>) -> Result<(), String> {
    let mut cache = Cache::default();
    check(fs, &mut cache)?;
    check_namespace(fs, &mut cache).map(|_| ())
}

/// Forty actions over `home` and `system`: files written, rewritten and
/// removed, so the live trees, the undo snapshots and the records share some
/// blocks and not others.
fn with_history() -> MemDisk {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [8; 16], "fuzz").unwrap();
    for n in 0..40u64 {
        fs.begin(n + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][n as usize % 2]).unwrap();
        let name = format!("file {}", n % 13);
        let f = match fs.lookup(ROOT, &name).unwrap() {
            Some(e) if n % 7 == 6 => {
                fs.remove(ROOT, &name).unwrap();
                fs.create(ROOT, &name).unwrap_or(e.ino)
            }
            Some(e) => e.ino,
            None => fs.create(ROOT, &name).unwrap(),
        };
        let len = 200 + (n as usize * 1777) % 9000;
        fs.write_file(f, &vec![n as u8 + 1; len]).unwrap();
        fs.commit().unwrap();
    }
    fs.into_dev()
}

fn damaged(rng: &mut XorShift, disk: &MemDisk, addr: u64, written: &[u64]) -> Block {
    let old = **disk.get(addr).expect("a written block");
    match rng.below(4) {
        0 => std::array::from_fn(|_| rng.next() as u8),
        1 => [0; BLOCK],
        2 => {
            let mut block = old;
            block[rng.below(BLOCK)] ^= 1 << rng.below(8);
            block
        }
        _ => **disk.get(written[rng.below(written.len())]).unwrap(),
    }
}

#[test]
fn random_damage_is_refused_at_mount_or_repaired_to_a_filesystem_that_checks() {
    let base = with_history();
    let slots: Vec<u64> = format::all_slots(BLOCKS).collect();
    let written: Vec<u64> = (0..BLOCKS)
        .filter(|a| base.get(*a).is_some() && !slots.contains(a))
        .collect();
    let cases = if cfg!(debug_assertions) { 120 } else { 1_200 };
    let mut rng = XorShift::new(27);
    let (mut refusals, mut repaired, mut sound) = (Vec::new(), 0, 0);
    for case in 0..cases {
        let mut disk = base.clone();
        for _ in 0..1 + rng.below(3) {
            let addr = written[rng.below(written.len())];
            let block = damaged(&mut rng, &disk, addr, &written);
            disk.put(addr, Rc::new(block));
        }
        let checked = Fs::mount(disk.clone())
            .map_err(|e| format!("mount: {e:?}"))
            .and_then(|mut fs| checks(&mut fs));
        let (mut fs, mut lines) = match mount_to_repair(disk) {
            Ok(mounted) => mounted,
            Err(e) => {
                refusals.push(format!("{e:?}"));
                continue;
            }
        };
        lines.extend(repair_fs(&mut fs).unwrap_or_else(|e| panic!("case {case}: repair: {e:?}")));
        match checked {
            Ok(()) => {
                assert_eq!(lines, Vec::<String>::new(), "case {case}: a sound disk");
                sound += 1;
                continue;
            }
            Err(why) => assert!(!lines.is_empty(), "case {case}: {why}, and no repair"),
        }
        let mut fs = Fs::mount(fs.into_dev())
            .unwrap_or_else(|e| panic!("case {case}: {e:?} after {lines:#?}"));
        checks(&mut fs).unwrap_or_else(|e| panic!("case {case}: {e} after {lines:#?}"));
        let again = repair_fs(&mut fs).unwrap();
        assert_eq!(again, Vec::<String>::new(), "case {case}: after {lines:#?}");
        repaired += 1;
    }
    eprintln!(
        "{cases} cases: {} refused, {repaired} repaired, {sound} sound; refused: {refusals:?}",
        refusals.len()
    );
    assert!(
        repaired > cases / 3,
        "only {repaired} of {cases} cases needed a repair"
    );
}
