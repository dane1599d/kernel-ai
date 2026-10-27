//! The namespace's rules (P2 task 5): what `check` adds above `check()`'s blocks,
//! on the live tree and on every snapshot.
//!
//! - Every item's object id is at least 1 and below the volume's counter, and an
//!   object's first item is its INODE, at offset 0. No other item kind exists in
//!   format 1.
//! - The root, inode 1, is a folder, its own parent, and has no name.
//! - DIRENT items belong to folders; their names are valid, sorted, distinct and
//!   filed under their own hash; each names an inode of the type it says.
//! - A file is at most 256 TiB. DATA items belong to files, start on a 128 KiB
//!   boundary, hold at least one byte and end within the file's size, and are
//!   inline exactly when their bytes fit an item.
//! - XATTR items' names are valid, sorted, distinct and filed under their hash.
//! - Every folder but the root has exactly one name, its parent is the folder
//!   holding it, its size counts its names, and its links are 2 plus its
//!   subfolders. Every file has a name; its links count its names, and its parent
//!   holds one of them.
//! - Every inode is reached from the root, so the folders form one tree.
//!
//! The report carries a **digest** of the namespace as a reader sees it: every
//! path, folder or file, every file's size and its 4 KiB pages that are not all
//! zero, and every attribute. It does not depend on how the data is laid out, so a
//! crash gate's model, which knows nothing of items, computes the same digest.
//! [`check_tree`] memoises a tree's verdict on its summary in the [`Cache`].

use crate::check::{Cache, Leaves, summarise};
use crate::dev::BlockDev;
use crate::format::BlockPtr;
use crate::fs::{Fs, MAX_INLINE, Snap, decode_value, inline_of, value_len};
use crate::items::{
    CHUNK, Entry, FileType, Inode, K_DATA, K_DIRENT, K_INODE, K_XATTR, MAX_FILE, ROOT,
    decode_attrs, decode_names, hash_name,
};
use crate::tree::Key;
use crate::vol::SnapKind;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A 4 KiB page of a file, as the digest counts it.
pub const PAGE: u64 = 4096;

/// What one tree's namespace holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NsReport {
    pub digest: Digest,
    pub files: u64,
    pub dirs: u64,
    /// The files' sizes, summed.
    pub bytes: u64,
    /// The highest object id an item has.
    pub max_oid: u64,
    /// The latest generation an inode says changed it.
    pub max_changed: u64,
}

/// A volume's namespace on its live tree and on each snapshot.
#[derive(Clone, Debug)]
pub struct VolumeNs {
    pub live: Rc<NsReport>,
    pub snaps: BTreeMap<u64, Rc<NsReport>>,
}

/// Every volume's namespace.
#[derive(Clone, Debug)]
pub struct NsCheck {
    pub volumes: BTreeMap<u64, VolumeNs>,
}

/// A namespace digest: a wrapping sum of item hashes, so order does not matter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Digest(pub u64);

impl Digest {
    fn add(&mut self, tag: u8, path: &str, payload: &[&[u8]]) {
        let mut buf = Vec::new();
        buf.push(tag);
        buf.extend_from_slice(&(path.len() as u64).to_le_bytes());
        buf.extend_from_slice(path.as_bytes());
        for part in payload {
            buf.extend_from_slice(&(part.len() as u64).to_le_bytes());
            buf.extend_from_slice(part);
        }
        self.0 = self.0.wrapping_add(seahash::hash(&buf));
    }

    pub fn dir(&mut self, path: &str) {
        self.add(1, path, &[]);
    }

    pub fn file(&mut self, path: &str, size: u64) {
        self.add(2, path, &[&size.to_le_bytes()]);
    }

    /// The pages `bytes` covers, starting at `at` (a multiple of [`PAGE`]), of a
    /// file of `size` bytes: each page that is not all zeros, cut at the size.
    pub fn bytes(&mut self, path: &str, size: u64, at: u64, bytes: &[u8]) {
        debug_assert_eq!(at % PAGE, 0);
        let mut page = [0u8; PAGE as usize];
        for (i, part) in bytes.chunks(PAGE as usize).enumerate() {
            let start = at + i as u64 * PAGE;
            if start >= size || part.iter().all(|&b| b == 0) {
                continue;
            }
            let len = (size - start).min(PAGE) as usize;
            page.fill(0);
            let n = part.len().min(len);
            page[..n].copy_from_slice(&part[..n]);
            self.add(3, path, &[&(start / PAGE).to_le_bytes(), &page[..len]]);
        }
    }

    pub fn attr(&mut self, path: &str, name: &str, value: &[u8]) {
        self.add(4, path, &[name.as_bytes(), value]);
    }
}

/// What the scan of one tree gathered.
#[derive(Default)]
struct Seen {
    inodes: BTreeMap<u64, Inode>,
    names: BTreeMap<u64, Vec<Entry>>,
    data: BTreeMap<u64, Vec<(u64, Vec<u8>)>>,
    attrs: BTreeMap<u64, Vec<(String, Vec<u8>)>>,
    max_oid: u64,
    max_changed: u64,
}

/// One item against the rules that need nothing but it and its inode.
fn item(
    seen: &mut Seen,
    current: &mut u64,
    key: &Key,
    raw: &[u8],
    hash: &dyn Fn(&str) -> u64,
) -> Result<(), String> {
    let oid = key.oid;
    let at = |what: &str| format!("object {oid}: {what}");
    if key.oid != *current {
        if key.kind != K_INODE {
            return Err(at("items, but no inode"));
        }
        *current = key.oid;
    }
    seen.max_oid = seen.max_oid.max(oid);
    let inline = |what: &str| inline_of(raw).ok_or_else(|| at(what));
    let kind = seen.inodes.get(&oid).map(|i| i.kind);
    match key.kind {
        K_INODE if key.off == 0 => {
            let inode = Inode::decode(inline("an inode held in a record")?)
                .map_err(|e| at(&format!("{e:?}")))?;
            if inode.kind == FileType::File && inode.size > MAX_FILE {
                return Err(at("a file over 256 TiB"));
            }
            seen.max_changed = seen.max_changed.max(inode.changed);
            seen.inodes.insert(oid, inode);
        }
        K_DIRENT if kind == Some(FileType::Dir) => {
            let names = decode_names(inline("names held in a record")?)
                .map_err(|e| at(&format!("{e:?}")))?;
            if names.iter().any(|e| hash(&e.name) != key.off) {
                return Err(at("a name filed under another hash"));
            }
            seen.names.entry(oid).or_default().extend(names);
        }
        K_DATA if kind == Some(FileType::File) => {
            let size = seen.inodes[&oid].size;
            let len = value_len(raw).ok_or_else(|| at("a data item"))? as u64;
            if !key.off.is_multiple_of(CHUNK) || len == 0 || key.off.saturating_add(len) > size {
                return Err(at(&format!(
                    "data at {} of {len} bytes, in a file of {size}",
                    key.off
                )));
            }
            if (len <= MAX_INLINE as u64) != inline_of(raw).is_some() {
                return Err(at("data inline that does not fit, or a record that would"));
            }
            seen.data
                .entry(oid)
                .or_default()
                .push((key.off, raw.to_vec()));
        }
        K_XATTR => {
            let attrs = decode_attrs(inline("attributes held in a record")?)
                .map_err(|e| at(&format!("{e:?}")))?;
            if attrs.iter().any(|(n, _)| hash(n) != key.off) {
                return Err(at("an attribute filed under another hash"));
            }
            seen.attrs.entry(oid).or_default().extend(attrs);
        }
        K_DIRENT | K_DATA => return Err(at("names in a file, or data in a folder")),
        _ => return Err(at(&format!("an item of kind {} at {}", key.kind, key.off))),
    }
    Ok(())
}

/// The rules that join items: names, links, parents, sizes, reachability.
fn links(seen: &Seen) -> Result<(), String> {
    let root = seen.inodes.get(&ROOT).ok_or("no root folder")?;
    if root.kind != FileType::Dir || root.parent != ROOT {
        return Err("the root is not a folder that is its own parent".to_string());
    }
    let mut named: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (dir, entries) in &seen.names {
        for e in entries {
            let target = seen
                .inodes
                .get(&e.ino)
                .ok_or_else(|| format!("folder {dir}: {:?} names no inode", e.name))?;
            if target.kind != e.kind {
                return Err(format!("folder {dir}: {:?} gives the wrong type", e.name));
            }
            named.entry(e.ino).or_default().push(*dir);
        }
    }
    for (ino, inode) in &seen.inodes {
        let by = named.get(ino).map_or(&[][..], |v| &v[..]);
        let fault = |what: &str| Err(format!("object {ino}: {what}"));
        match inode.kind {
            FileType::Dir => {
                let entries = seen.names.get(ino).map_or(&[][..], |v| &v[..]);
                let subdirs = entries.iter().filter(|e| e.kind == FileType::Dir).count() as u64;
                if inode.size != entries.len() as u64 {
                    return fault("a folder's size is not its names");
                }
                if u64::from(inode.nlink) != 2 + subdirs {
                    return fault("a folder's links are not 2 plus its subfolders");
                }
                let names_wanted = usize::from(*ino != ROOT);
                if by.len() != names_wanted || (*ino != ROOT && by[0] != inode.parent) {
                    return fault("a folder named other than once, by its parent");
                }
            }
            FileType::File => {
                if by.is_empty() || u64::from(inode.nlink) != by.len() as u64 {
                    return fault("a file's links are not its names");
                }
                if !by.contains(&inode.parent) {
                    return fault("a file's parent does not name it");
                }
            }
        }
    }
    Ok(())
}

/// Walks the namespace from the root, reading every file, into a digest.
fn walk<D: BlockDev>(fs: &mut Fs<D>, seen: &Seen) -> Result<NsReport, String> {
    let mut report = NsReport {
        max_oid: seen.max_oid,
        max_changed: seen.max_changed,
        ..NsReport::default()
    };
    let mut digest = Digest::default();
    let mut reached = BTreeSet::new();
    let mut queue: VecDeque<(u64, String)> = VecDeque::new();
    queue.push_back((ROOT, "/".to_string()));
    while let Some((ino, path)) = queue.pop_front() {
        reached.insert(ino);
        let inode = seen.inodes[&ino];
        for (name, value) in seen.attrs.get(&ino).into_iter().flatten() {
            digest.attr(&path, name, value);
        }
        match inode.kind {
            FileType::Dir => {
                report.dirs += 1;
                digest.dir(&path);
                for e in seen.names.get(&ino).into_iter().flatten() {
                    let child = match path.as_str() {
                        "/" => format!("/{}", e.name),
                        _ => format!("{path}/{}", e.name),
                    };
                    queue.push_back((e.ino, child));
                }
            }
            FileType::File => {
                report.files += 1;
                report.bytes = report.bytes.saturating_add(inode.size);
                digest.file(&path, inode.size);
                for (at, raw) in seen.data.get(&ino).into_iter().flatten() {
                    let value = decode_value(raw, &mut fs.io)
                        .map_err(|e| format!("{path}: data at {at}: {e:?}"))?;
                    digest.bytes(&path, inode.size, *at, value.bytes());
                }
            }
        }
    }
    if reached.len() != seen.inodes.len() {
        return Err(format!(
            "{} of {} inodes are not reached from the root",
            seen.inodes.len() - reached.len(),
            seen.inodes.len()
        ));
    }
    report.digest = digest;
    Ok(report)
}

/// The namespace rules on the tree `root` names, under the counter `next_oid`.
pub fn rules<D: BlockDev>(
    fs: &mut Fs<D>,
    root: &BlockPtr,
    next_oid: u64,
) -> Result<NsReport, String> {
    if root.is_null() {
        return Err("no root folder: the tree is empty".to_string());
    }
    let (uuid, mask) = (fs.uuid(), fs.hash_mask);
    let hash = move |name: &str| hash_name(&uuid, mask, name);
    let mut seen = Seen::default();
    let mut current = 0;
    let mut failed = None;
    fs.scan_tree(root, &Key::MIN, &mut |key, raw| {
        if let Err(e) = item(&mut seen, &mut current, key, raw, &hash) {
            failed = Some(e);
        }
        failed.is_none()
    })
    .map_err(|e| format!("namespace scan: {e:?}"))?;
    if let Some(e) = failed {
        return Err(e);
    }
    if seen.inodes.keys().next().is_some_and(|&first| first < ROOT) || seen.max_oid >= next_oid {
        return Err(format!(
            "an object id outside 1..{next_oid}: {}",
            seen.max_oid
        ));
    }
    links(&seen)?;
    walk(fs, &seen)
}

/// [`rules`] through the memo: a tree whose summary still holds is not read
/// again. A tree from generation `generation` may hold no object above its
/// counter `next_oid`, and no inode changed after it.
pub fn check_tree<D: BlockDev>(
    fs: &mut Fs<D>,
    root: &BlockPtr,
    next_oid: u64,
    generation: u64,
    cache: &mut Cache,
) -> Result<Rc<NsReport>, String> {
    let report = match cache.namespace(root) {
        Some(report) => report,
        None => {
            if !root.is_null() {
                summarise(&mut fs.io, root, Leaves::Values, cache)?;
            }
            let report = Rc::new(rules(fs, root, next_oid)?);
            cache.put_namespace(root, report.clone());
            report
        }
    };
    if report.max_changed > generation {
        return Err(format!(
            "an inode changed in {}, after its tree's generation {generation}",
            report.max_changed
        ));
    }
    match report.max_oid < next_oid {
        true => Ok(report),
        false => Err(format!(
            "an object id outside 1..{next_oid}: {}",
            report.max_oid
        )),
    }
}

/// The namespace rules on every volume's live tree, as last committed, and on
/// every snapshot; and the UNDO records' rules: each decodes, follows the
/// generation before it, names an undo snapshot of that generation, and names
/// only objects below the volume's counter.
pub fn check_namespace<D: BlockDev>(fs: &mut Fs<D>, cache: &mut Cache) -> Result<NsCheck, String> {
    if !fs.is_clean() {
        return Err("check runs between commits".to_string());
    }
    let active = fs.volume();
    let mut volumes = BTreeMap::new();
    for (id, name) in fs.volumes() {
        let found = volume(fs, id, cache).map_err(|e| format!("volume {name}: {e}"));
        let _ = fs.set_volume(active);
        volumes.insert(id, found?);
    }
    Ok(NsCheck { volumes })
}

fn volume<D: BlockDev>(fs: &mut Fs<D>, id: u64, cache: &mut Cache) -> Result<VolumeNs, String> {
    let at = |e: crate::Error| format!("{e:?}");
    fs.set_volume(id).map_err(at)?;
    let live_root = fs.live_root().ok_or("the live tree is not written")?;
    let (next_oid, committed) = (fs.next_oid(), fs.committed);
    let live = check_tree(fs, &live_root, next_oid, committed, cache)?;
    let roots: Vec<(u64, Snap)> = fs.snapshots().iter().map(|(sid, s)| (*sid, *s)).collect();
    let mut snaps = BTreeMap::new();
    for (sid, snap) in &roots {
        let report = check_tree(fs, &snap.root, snap.next_oid, snap.generation, cache)
            .map_err(|e| format!("snapshot {sid}: {e}"))?;
        snaps.insert(*sid, report);
    }
    for undo in fs.undo_records().map_err(at)? {
        let snap = roots
            .iter()
            .find(|(sid, _)| *sid == undo.snapshot)
            .map(|(_, s)| s);
        let sound = snap.is_some_and(|s| s.kind == SnapKind::Undo && s.generation == undo.before)
            && undo.before < undo.txn
            && undo.txn <= committed
            && undo.objects.iter().all(|&o| (1..next_oid).contains(&o));
        if !sound {
            return Err(format!("the undo record of {} does not hold", undo.txn));
        }
    }
    Ok(VolumeNs { live, snaps })
}
