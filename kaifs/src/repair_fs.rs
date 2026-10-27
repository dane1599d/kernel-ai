//! `check --repair` on a whole filesystem: the space and history rules, then
//! each volume's namespace rules ([`crate::repair`]), each step its own commit.
//!
//! 0. **Space.** Before anything is written, every block still reached, as far
//!    as its nodes read, is taken out of free and pending space: each volume's
//!    live tree and snapshots with their records, the volume table and the
//!    allocator's chain. No repair write can then land on a block something
//!    names, and the commit writes the allocator's base anew. A pending run
//!    that overlaps is dropped whole; the rest of it is the GC's. A disk that
//!    does not mount is mounted by [`mount_to_repair`] from the newest commit
//!    whose volume table reads, its allocator rebuilt from what every commit on
//!    the rings reaches.
//! 7. **History.** A snapshot whose blocks or namespace do not check is
//!    dropped, with the UNDO records it backs, and so is an UNDO record that
//!    does not decode or hold. Those actions lose their undo; nothing else
//!    changes. The blocks wait for the GC.
//!
//! Then rules 1-6 run on each volume's live tree. It refuses what
//! [`repair`] refuses.

use crate::Error;
use crate::allocator::{Allocator, run_of};
use crate::buddy::Run;
use crate::check::Cache;
use crate::csum::Csum;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr};
use crate::fs::{Faults, Fs, Io, Snap, record_of};
use crate::ns_check::{check_tree, undo_holds};
use crate::repair::repair;
use crate::tree::{Child, Node};
use crate::vol::decode_undo;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Applies every rule and commits what each changed. Returns one line a change,
/// a volume's lines led by its name; none if the filesystem kept them all.
pub fn repair_fs<D: BlockDev>(fs: &mut Fs<D>) -> Result<Vec<String>, Error> {
    if !fs.writable() {
        return Err(Error::ReadOnly);
    }
    if !fs.repairable() {
        return Err(Error::Unsupported(
            "an unknown compat feature: repair refuses",
        ));
    }
    if !fs.is_clean() {
        return Err(Error::Invalid("an open transaction"));
    }
    let active = fs.volume();
    let result = all(fs);
    fs.set_volume(active)?;
    result
}

fn all<D: BlockDev>(fs: &mut Fs<D>) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    let taken = space(fs);
    if taken > 0 {
        lines.push(format!(
            "{taken} blocks something reaches were free or pending: taken back"
        ));
        fs.commit()?;
    }
    for (id, name) in fs.volumes() {
        fs.set_volume(id)?;
        let mut fixes = history(fs)?;
        fixes.extend(repair(fs)?);
        lines.extend(fixes.into_iter().map(|fix| format!("{name}: {fix}")));
    }
    Ok(lines)
}

/// Mounts `dev` for `check --repair`: as [`Fs::mount`] does, or, when that
/// fails, with the allocator rebuilt (rule 0) under the newest commit whose
/// volume table reads. Every block that a commit on the rings still reaches,
/// as far as it reads, is in use, and the rest of the data area is free; the
/// rebuilt allocator is committed at once, above every commit on the rings, so
/// the disk mounts again even if the repair stops there. Returns the line
/// saying so.
pub fn mount_to_repair<D: BlockDev>(mut dev: D) -> Result<(Fs<D>, Vec<String>), Error> {
    let faults = Faults::default();
    let error = match Fs::newest(&mut dev) {
        Ok((sb, rules)) => match Fs::load(&mut dev, &sb, faults) {
            Ok(loaded) => return Ok((Fs::assemble(dev, &sb, rules, loaded, faults), Vec::new())),
            Err(e) => e,
        },
        Err(e) => e,
    };
    let (supers, rules) = Fs::with_tables(&mut dev)?;
    if !rules.repairable {
        return Err(Error::Unsupported(
            "an unknown compat feature: repair refuses",
        ));
    }
    let area = format::data_area(dev.info().blocks);
    let mut runs: Vec<Run> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut chosen = None;
    for sb in &supers {
        let chain = Allocator::chain_blocks(&mut dev, sb.csum_kind, &sb.alloc);
        runs.extend(chain.iter().map(run_of));
        let Ok(loaded) = Fs::load_tables(&mut dev, sb, Allocator::new(area), faults) else {
            continue;
        };
        reached(
            &mut Fs::assemble(&mut dev, sb, rules, loaded, faults),
            &mut runs,
            &mut seen,
        );
        chosen = chosen.or(Some(*sb));
    }
    let sb = chosen.ok_or(error)?;
    let loaded = Fs::load_tables(&mut dev, &sb, Allocator::new(area), faults)?;
    let mut fs = Fs::assemble(dev, &sb, rules, loaded, faults);
    if !fs.writable() {
        return Err(Error::ReadOnly);
    }
    let used = fs.io.alloc.claim(&runs);
    // Above every commit on the rings, so a mount takes this one.
    let newest = format::read_rings(&mut fs.io.dev)?
        .first()
        .map_or(sb.generation, |s| s.generation);
    fs.io.txn = fs.io.txn.max(newest + 1);
    fs.commit()?;
    let line = format!(
        "the disk does not mount ({error:?}): generation {} mounted, the newest whose volume table reads, its allocator rebuilt from what {} commits reach, {used} blocks in use",
        sb.generation,
        supers.len()
    );
    Ok((fs, alloc::vec![line]))
}

/// Rule 0. Returns the blocks taken back.
fn space<D: BlockDev>(fs: &mut Fs<D>) -> u64 {
    let mut runs: Vec<Run> = Vec::new();
    reached(fs, &mut runs, &mut BTreeSet::new());
    fs.io.alloc.claim(&runs)
}

/// Adds to `runs` every run `fs` reaches, as far as it reads: the allocator's
/// chain, the volume table, and each volume's live tree and snapshots with
/// their records.
fn reached<D: BlockDev>(fs: &mut Fs<D>, runs: &mut Vec<Run>, seen: &mut BTreeSet<(u64, Csum)>) {
    runs.extend(fs.io.alloc.chain().iter().map(run_of));
    if let Some(root) = fs.volumes_root() {
        reach(&mut fs.io, &root, false, runs, seen);
    }
    for (id, _) in fs.volumes() {
        let mut roots: Vec<BlockPtr> = fs.live_root_of(id).into_iter().collect();
        if let Some((_, snaps)) = fs.volume_state(id) {
            roots.extend(snaps.values().map(|s| s.root));
        }
        for root in roots {
            reach(&mut fs.io, &root, true, runs, seen);
        }
    }
}

/// Every run the tree `ptr` names reaches, as far as its nodes read: its nodes
/// and, in a tree of `values`, their records. Each pointer is followed once, so
/// a hostile tree that names a node twice, or itself, ends.
fn reach<D: BlockDev>(
    io: &mut Io<D>,
    ptr: &BlockPtr,
    values: bool,
    runs: &mut Vec<Run>,
    seen: &mut BTreeSet<(u64, Csum)>,
) {
    if ptr.is_null() || !seen.insert((ptr.addr, ptr.csum)) {
        return;
    }
    runs.push(run_of(ptr));
    match io.load_node(ptr) {
        Err(_) => {}
        Ok(Node::Leaf(items)) if values => {
            let records = items.iter().filter_map(|(_, raw)| record_of(raw));
            runs.extend(records.map(|(record, _)| run_of(&record)));
        }
        Ok(Node::Leaf(_)) => {}
        Ok(Node::Branch(_, children)) => {
            for (_, child) in children {
                if let Child::Clean(child) = child {
                    reach(io, &child, values, runs, seen);
                }
            }
        }
    }
}

/// Rule 7, on the active volume.
fn history<D: BlockDev>(fs: &mut Fs<D>) -> Result<Vec<String>, Error> {
    let mut fixes = Vec::new();
    let mut cache = Cache::default();
    let (next_oid, committed) = (fs.next_oid(), fs.committed);
    let snaps: Vec<(u64, Snap)> = fs.snapshots().iter().map(|(id, s)| (*id, *s)).collect();
    let mut bad = BTreeSet::new();
    for (sid, snap) in &snaps {
        if let Err(why) = check_tree(fs, &snap.root, snap.next_oid, snap.generation, &mut cache) {
            fixes.push(format!("snapshot {sid} does not check ({why}): dropped"));
            bad.insert(*sid);
        }
    }
    let mut kept = fs.snapshots().clone();
    kept.retain(|id, _| !bad.contains(id));
    for (txn, parts) in fs.undo_raw()? {
        let numbers: Vec<u64> = parts.iter().map(|(part, _)| *part).collect();
        let whole = numbers
            .iter()
            .enumerate()
            .all(|(i, &part)| part == i as u64);
        let values: Vec<Vec<u8>> = parts.into_iter().map(|(_, value)| value).collect();
        let holds = whole
            && decode_undo(txn, &values).is_ok_and(|u| undo_holds(&u, &kept, committed, next_oid));
        if !holds {
            fs.drop_undo(txn, &numbers)?;
            fixes.push(format!("the undo record of {txn} does not hold: dropped"));
        }
    }
    for sid in bad {
        fs.drop_snapshot(sid)?;
    }
    if !fixes.is_empty() {
        fs.commit()?;
    }
    Ok(fixes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::check;
    use crate::fs::HOME;
    use crate::io::Meta;
    use crate::items::ROOT;
    use crate::ns_check::check_namespace;
    use crate::tree::Tree;
    use crate::vol::{encode_undo, undo_key};
    use crate::{BLOCK, MemDisk};
    use alloc::rc::Rc;
    use alloc::vec;

    /// Sixty actions in `home`, each a new file: sixty undo snapshots and UNDO
    /// records, and a live tree over two levels.
    fn populated() -> Fs<MemDisk> {
        let mut fs = Fs::mkfs(MemDisk::new(4096), [3; 16], "repair").unwrap();
        fs.set_volume(HOME).unwrap();
        for i in 0..60u64 {
            fs.begin(i + 1).unwrap();
            let f = fs.create(ROOT, &format!("file {i}")).unwrap();
            fs.write_file(f, &vec![i as u8; 300 + i as usize * 50])
                .unwrap();
            fs.commit().unwrap();
        }
        fs
    }

    fn checks(fs: &mut Fs<MemDisk>) -> Result<(), String> {
        let mut cache = Cache::default();
        check(fs, &mut cache)?;
        check_namespace(fs, &mut cache).map(|_| ())
    }

    fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
        let mut fs = Fs::mount(fs.into_dev()).unwrap();
        fs.set_volume(HOME).unwrap();
        fs
    }

    /// Repairs `fs`, which must not check; after, it must, and a second repair
    /// finds nothing.
    fn repaired(fs: Fs<MemDisk>) -> (Fs<MemDisk>, Vec<String>) {
        let mut fs = remount(fs);
        assert!(checks(&mut fs).is_err(), "the damage went unnoticed");
        let lines = repair_fs(&mut fs).unwrap();
        let mut fs = remount(fs);
        checks(&mut fs).unwrap_or_else(|e| panic!("{e} after {lines:#?}"));
        assert_eq!(repair_fs(&mut fs).unwrap(), Vec::<String>::new());
        (fs, lines)
    }

    #[test]
    fn a_sound_filesystem_is_left_alone() {
        let mut fs = remount(populated());
        assert_eq!(repair_fs(&mut fs).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn blocks_a_tree_reaches_but_the_allocator_holds_unused_are_taken_back() {
        let mut fs = populated();
        let root = fs.live_root().unwrap();
        let txn = fs.io.txn;
        fs.io.alloc.free_later(txn, run_of(&root));
        fs.commit().unwrap();
        let (_, lines) = repaired(fs);
        assert_eq!(
            lines,
            ["1 blocks something reaches were free or pending: taken back"]
        );
    }

    #[test]
    fn an_allocator_that_does_not_read_is_rebuilt_from_what_the_commits_reach() {
        let fs = populated();
        let chain = fs.io.alloc.chain().to_vec();
        let mut dev = fs.into_dev();
        // A delta in the middle of the chain: no commit's allocator reads.
        dev.put(chain[chain.len() / 2].addr, Rc::new([0x33; BLOCK]));
        assert!(Fs::mount(dev.clone()).is_err());
        let (mut fs, lines) = mount_to_repair(dev).unwrap();
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert!(
            lines[0].starts_with("the disk does not mount"),
            "{lines:#?}"
        );
        assert_eq!(repair_fs(&mut fs).unwrap(), Vec::<String>::new());
        let mut fs = remount(fs);
        checks(&mut fs).unwrap();
        // It writes again without touching what the trees hold.
        let f = fs.create(ROOT, "after").unwrap();
        fs.write_file(f, &vec![9; 20_000]).unwrap();
        fs.commit().unwrap();
        checks(&mut remount(fs)).unwrap();
    }

    #[test]
    fn a_snapshot_that_does_not_check_is_dropped_with_its_undo_record() {
        let mut fs = populated();
        let undo = fs.undo_records().unwrap()[0].clone();
        let root = fs.snapshots()[&undo.snapshot].root;
        fs.io.dev.put(root.addr, Rc::new([0x5A; BLOCK]));
        let (mut fs, lines) = repaired(fs);
        assert_eq!(lines.len(), 2, "{lines:#?}");
        let dropped = format!("home: snapshot {} does not check", undo.snapshot);
        assert!(lines[0].starts_with(&dropped), "{lines:#?}");
        assert_eq!(
            lines[1],
            format!(
                "home: the undo record of {} does not hold: dropped",
                undo.txn
            )
        );
        assert!(!fs.snapshots().contains_key(&undo.snapshot));
        assert_eq!(fs.undo_record(undo.txn).unwrap(), None);
        assert_eq!(fs.undo_records().unwrap().len(), 59);
    }

    #[test]
    fn an_undo_record_that_does_not_decode_is_dropped() {
        let mut fs = populated();
        let txn = fs.undo_records().unwrap()[10].txn;
        fs.volumes
            .insert(
                undo_key(HOME, txn, 0),
                vec![0xEE; 40],
                &mut Meta(&mut fs.io),
            )
            .unwrap();
        fs.commit().unwrap();
        let (mut fs, lines) = repaired(fs);
        let want = format!("home: the undo record of {txn} does not hold: dropped");
        assert_eq!(lines, [want]);
        assert_eq!(fs.undo_records().unwrap().len(), 59);
    }

    #[test]
    fn an_undo_record_naming_an_object_past_the_counter_is_dropped() {
        let mut fs = populated();
        let mut undo = fs.undo_records().unwrap()[5].clone();
        undo.objects.push(fs.next_oid() + 10);
        for (part, value) in encode_undo(&undo).unwrap().into_iter().enumerate() {
            let key = undo_key(HOME, undo.txn, part as u64);
            fs.volumes
                .insert(key, value, &mut Meta(&mut fs.io))
                .unwrap();
        }
        fs.commit().unwrap();
        let (fs, lines) = repaired(fs);
        let want = format!(
            "home: the undo record of {} does not hold: dropped",
            undo.txn
        );
        assert_eq!(lines, [want]);
        // Its snapshot stays, for retention to drop.
        assert!(fs.snapshots().contains_key(&undo.snapshot));
    }

    #[test]
    fn a_live_tree_whose_nodes_verify_but_do_not_hold_together_is_rebuilt() {
        let mut fs = populated();
        let digest = |fs: &mut Fs<MemDisk>| {
            let ns = check_namespace(fs, &mut Cache::default()).unwrap();
            ns.volumes[&HOME].live.digest
        };
        let before = digest(&mut fs);
        // The first leaf split in two: one item, under a quarter full and not
        // the last, then the rest.
        let root = fs.live_root().unwrap();
        let Node::Branch(level, mut children) = fs.io.load_node(&root).unwrap() else {
            panic!("a live tree of one leaf");
        };
        let Child::Clean(first) = children[0].1 else {
            unreachable!("a node read from disk is clean");
        };
        let Node::Leaf(mut items) = fs.io.load_node(&first).unwrap() else {
            panic!("a live tree over three levels");
        };
        let rest = items.split_off(1);
        let rest_key = rest[0].0;
        let a = fs.io.store_node(&Node::Leaf(items).encode(), 0).unwrap();
        let b = fs.io.store_node(&Node::Leaf(rest).encode(), 0).unwrap();
        children[0].1 = Child::Clean(a);
        children.insert(1, (rest_key, Child::Clean(b)));
        let root = fs
            .io
            .store_node(&Node::Branch(level, children).encode(), level)
            .unwrap();
        let vol = fs.vol_mut();
        vol.live = Tree::open(root);
        vol.live_changed = true;
        fs.commit().unwrap();
        let (mut fs, lines) = repaired(fs);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert!(lines[0].starts_with("home: the live tree does not check"));
        assert!(lines[0].ends_with("rebuilt from its items"));
        assert_eq!(digest(&mut fs), before);
    }
}
