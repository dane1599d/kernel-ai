//! `check --repair`'s rules (P2 task 5). When the namespace rules fail, repair
//! writes, as one new commit, the nearest namespace that keeps them, and says
//! what it changed, one line a change:
//!
//! 1. **Salvage.** Every item the live tree still yields is kept; a node that no
//!    longer verifies loses the items below it, and the tree is rebuilt from the
//!    rest. The old tree's blocks wait for the GC, so nothing a snapshot holds is
//!    touched.
//! 2. **Items.** An item of an unknown kind, of object 0, or of an object with no
//!    inode is dropped, as are names in a file, data in a folder, data off a
//!    128 KiB boundary, past 256 TiB or in a record that no longer verifies, and
//!    an item that does not decode. A name or attribute filed under another hash is filed
//!    again under its own. Data is made inline exactly when it fits. The object
//!    id counter is raised past every object found.
//! 3. **The root.** If inode 1 is missing or not a folder, it becomes an empty
//!    folder again.
//! 4. **Names.** A name of an inode that is missing or of another type is
//!    dropped. Walking from the root, a second name for a folder is dropped, so
//!    the folders form a tree again.
//! 5. **Orphans.** An inode the walk did not reach is named by its number in
//!    `/lost+found`, which is made if missing, lowest number first, with what it
//!    holds.
//! 6. **Counts.** Every size, link count and parent is set from the names, a
//!    file's size covers its data, and no inode says it changed after the commit
//!    repair writes.
//!
//! Repair acts on the active volume, so `check --repair` runs it once a volume.
//! It refuses a filesystem with an unknown compat bit (ADR 0004 §2), one
//! mounted read-only, and one with an open transaction. Space the allocator
//! accounts wrongly is the GC's (task 6), which rebuilds it from what is reached.

use crate::Error;
use crate::dev::BlockDev;
use crate::format::BlockPtr;
use crate::fs::{Fs, MAX_INLINE, decode_value, inline_of, inline_raw, record_of, value_len};
use crate::items::{
    CHUNK, Entry, FileType, Inode, K_DATA, K_DIRENT, K_INODE, K_XATTR, MAX_FILE, ROOT,
    decode_attrs, decode_names, encode_attrs, encode_names, hash_name,
};
use crate::tree::{Child, Key, Node};
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const LOST: &str = "lost+found";

/// An object's attributes, by name hash.
type Attrs = BTreeMap<u64, Vec<(String, Vec<u8>)>>;

/// The namespace as repair rebuilds it.
#[derive(Default)]
struct State {
    inodes: BTreeMap<u64, Inode>,
    /// Each folder's names, by hash.
    names: BTreeMap<u64, BTreeMap<u64, Vec<Entry>>>,
    /// Each file's data items: offset, raw value.
    data: BTreeMap<u64, BTreeMap<u64, Vec<u8>>>,
    attrs: BTreeMap<u64, Attrs>,
}

struct Repair<'a, D> {
    fs: &'a mut Fs<D>,
    state: State,
    fixes: Vec<String>,
    uuid: [u8; 16],
    mask: u64,
    /// `/lost+found`, once found or made.
    lost: Option<u64>,
}

/// Every item under `ptr` that still verifies; notes each node that does not.
fn salvage<D: BlockDev>(
    fs: &mut Fs<D>,
    ptr: &BlockPtr,
    items: &mut Vec<(Key, Vec<u8>)>,
    fixes: &mut Vec<String>,
) {
    match fs.io.load_node(ptr) {
        Err(e) => fixes.push(format!(
            "node {} does not verify ({e:?}): the items below it are lost",
            ptr.addr
        )),
        Ok(Node::Leaf(leaf)) => items.extend(leaf),
        Ok(Node::Branch(_, children)) => {
            for (_, child) in children {
                if let Child::Clean(child) = child {
                    salvage(fs, &child, items, fixes);
                }
            }
        }
    }
}

/// Puts `entry` in `chains` under `hash`, if the name is free and the item can
/// hold it.
fn file_name(chains: &mut BTreeMap<u64, Vec<Entry>>, hash: u64, entry: Entry) -> bool {
    let chain = chains.entry(hash).or_default();
    let Err(at) = chain.binary_search_by(|e| e.name.cmp(&entry.name)) else {
        return false;
    };
    chain.insert(at, entry);
    if encode_names(chain).len() > MAX_INLINE {
        chain.remove(at);
        return false;
    }
    true
}

impl<D: BlockDev> Repair<'_, D> {
    fn hash(&self, name: &str) -> u64 {
        hash_name(&self.uuid, self.mask, name)
    }

    fn fix(&mut self, line: String) {
        self.fixes.push(line);
    }

    /// Rule 2, one item at a time.
    fn item(&mut self, key: Key, raw: Vec<u8>) {
        let oid = key.oid;
        let kind = self.state.inodes.get(&oid).map(|i| i.kind);
        let dropped = |what: &str| format!("object {oid}: dropped {what} at {}", key.off);
        if oid == 0 || (key.kind != K_INODE && kind.is_none()) {
            self.fix(dropped(&format!(
                "an item of kind {} with no inode",
                key.kind
            )));
            return;
        }
        match key.kind {
            K_INODE if key.off == 0 => match inline_of(&raw).map(Inode::decode) {
                Some(Ok(inode)) => {
                    self.state.inodes.insert(oid, inode);
                }
                _ => self.fix(dropped("an inode that does not decode")),
            },
            K_DIRENT if kind == Some(FileType::Dir) => {
                let Some(Ok(names)) = inline_of(&raw).map(decode_names) else {
                    return self.fix(dropped("names that do not decode"));
                };
                for e in names {
                    let hash = self.hash(&e.name);
                    if hash != key.off {
                        self.fix(format!(
                            "folder {oid}: {:?} filed again under its hash",
                            e.name
                        ));
                    }
                    let name = e.name.clone();
                    let chains = self.state.names.entry(oid).or_default();
                    if !file_name(chains, hash, e) {
                        self.fix(format!("folder {oid}: dropped a second {name:?}"));
                    }
                }
            }
            K_DATA if kind == Some(FileType::File) => self.data(key, raw),
            K_XATTR => {
                let Some(Ok(attrs)) = inline_of(&raw).map(decode_attrs) else {
                    return self.fix(dropped("attributes that do not decode"));
                };
                for (name, value) in attrs {
                    let hash = self.hash(&name);
                    if hash != key.off {
                        self.fix(format!(
                            "object {oid}: attribute {name:?} filed again under its hash"
                        ));
                    }
                    let chain = self
                        .state
                        .attrs
                        .entry(oid)
                        .or_default()
                        .entry(hash)
                        .or_default();
                    match chain.binary_search_by(|(n, _)| n.cmp(&name)) {
                        Ok(_) => {
                            self.fix(format!("object {oid}: dropped a second attribute {name:?}"))
                        }
                        Err(at) => chain.insert(at, (name, value)),
                    }
                }
            }
            _ => self.fix(dropped(&format!("an item of kind {}", key.kind))),
        }
    }

    /// Rule 2 for data: aligned, verifying, inline exactly when it fits.
    fn data(&mut self, key: Key, raw: Vec<u8>) {
        let (oid, at) = (key.oid, key.off);
        let dropped = |what: &str| format!("file {oid}: dropped {what} at {at}");
        let len = value_len(&raw).unwrap_or(0);
        if !at.is_multiple_of(CHUNK) || len == 0 || at.saturating_add(len as u64) > MAX_FILE {
            return self.fix(dropped(
                "data off a 128 KiB boundary, empty, or past 256 TiB",
            ));
        }
        let bytes = match decode_value(&raw, &mut self.fs.io) {
            Ok(value) => value.into_bytes(),
            Err(e) => return self.fix(dropped(&format!("data that does not verify ({e:?})"))),
        };
        let raw = match (len <= MAX_INLINE, record_of(&raw).is_some()) {
            (true, true) => {
                self.fix(format!("file {oid}: data at {at} made inline"));
                inline_raw(&bytes)
            }
            (false, false) => return self.fix(dropped("inline data over what an item holds")),
            _ => raw,
        };
        self.state.data.entry(oid).or_default().insert(at, raw);
    }

    /// Rule 3.
    fn root(&mut self) {
        match self.state.inodes.get(&ROOT).map(|i| i.kind) {
            Some(FileType::Dir) => {}
            found => {
                if found.is_some() {
                    self.fix("the root was a file: its data is dropped".to_string());
                    self.state.data.remove(&ROOT);
                }
                self.fix("the root folder made again".to_string());
                let now = self.fs.clock;
                self.state
                    .inodes
                    .insert(ROOT, Inode::new(FileType::Dir, ROOT, now));
            }
        }
    }

    /// Rule 4's first half: names of missing inodes, or of the wrong type.
    fn dangling(&mut self) {
        let inodes = &self.state.inodes;
        let mut fixes = Vec::new();
        self.state.names.retain(|dir, chains| {
            if inodes.get(dir).map(|i| i.kind) != Some(FileType::Dir) {
                return false;
            }
            for chain in chains.values_mut() {
                chain.retain(|e| {
                    let ok = inodes.get(&e.ino).is_some_and(|i| i.kind == e.kind);
                    if !ok {
                        fixes.push(format!(
                            "folder {dir}: dropped {:?}, which names no such inode",
                            e.name
                        ));
                    }
                    ok
                });
            }
            chains.retain(|_, chain| !chain.is_empty());
            true
        });
        self.state
            .data
            .retain(|ino, _| inodes.get(ino).map(|i| i.kind) == Some(FileType::File));
        self.state.attrs.retain(|ino, _| inodes.contains_key(ino));
        self.fixes.extend(fixes);
    }

    /// Rule 4's second half and rule 5: walks from the root, then from each
    /// orphan, dropping second names for folders. Returns each inode's names.
    fn walk(&mut self) -> Result<BTreeMap<u64, Vec<u64>>, Error> {
        let mut named: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        let mut visited: BTreeSet<u64> = BTreeSet::new();
        let mut start = Some(ROOT);
        while let Some(top) = start {
            visited.insert(top);
            let mut queue = VecDeque::from([top]);
            while let Some(dir) = queue.pop_front() {
                let Some(chains) = self.state.names.get_mut(&dir) else {
                    continue;
                };
                let mut fixes = Vec::new();
                for chain in chains.values_mut() {
                    chain.retain(|e| {
                        let second = e.kind == FileType::Dir && visited.contains(&e.ino);
                        if second {
                            fixes.push(format!(
                                "folder {dir}: dropped {:?}, a second name for folder {}",
                                e.name, e.ino
                            ));
                            return false;
                        }
                        visited.insert(e.ino);
                        named.entry(e.ino).or_default().push(dir);
                        if e.kind == FileType::Dir {
                            queue.push_back(e.ino);
                        }
                        true
                    });
                }
                chains.retain(|_, chain| !chain.is_empty());
                self.fixes.extend(fixes);
            }
            start = self
                .state
                .inodes
                .keys()
                .find(|ino| !visited.contains(ino))
                .copied();
            if let Some(orphan) = start {
                let lost = self.lost_found(&mut named, &mut visited)?;
                let kind = self.state.inodes[&orphan].kind;
                self.adopt(lost, orphan, kind)?;
                named.entry(orphan).or_default().push(lost);
            }
        }
        Ok(named)
    }

    /// `/lost+found`, made if missing.
    fn lost_found(
        &mut self,
        named: &mut BTreeMap<u64, Vec<u64>>,
        visited: &mut BTreeSet<u64>,
    ) -> Result<u64, Error> {
        if let Some(lost) = self.lost {
            return Ok(lost);
        }
        let hash = self.hash(LOST);
        let found = self
            .state
            .names
            .get(&ROOT)
            .and_then(|c| c.get(&hash))
            .and_then(|chain| chain.iter().find(|e| e.name == LOST))
            .cloned();
        let lost = match found {
            Some(e) if e.kind == FileType::Dir => e.ino,
            Some(_) => return Err(Error::Invalid("/lost+found is a file: rename it first")),
            None => {
                let ino = self.fs.alloc_oid()?;
                let now = self.fs.clock;
                self.state
                    .inodes
                    .insert(ino, Inode::new(FileType::Dir, ROOT, now));
                self.adopt(ROOT, ino, FileType::Dir)?;
                visited.insert(ino);
                named.entry(ino).or_default().push(ROOT);
                self.fix("made /lost+found".to_string());
                ino
            }
        };
        self.lost = Some(lost);
        Ok(lost)
    }

    /// Names `ino` by its number in the folder `dir`.
    fn adopt(&mut self, dir: u64, ino: u64, kind: FileType) -> Result<(), Error> {
        let base = match dir == ROOT {
            true => LOST.to_string(),
            false => format!("{ino}"),
        };
        for n in 0..64 {
            let name = match n {
                0 => base.clone(),
                _ => format!("{base}~{n}"),
            };
            let hash = self.hash(&name);
            let chains = self.state.names.entry(dir).or_default();
            let entry = Entry {
                name: name.clone(),
                ino,
                kind,
            };
            if file_name(chains, hash, entry) {
                if dir != ROOT {
                    self.fix(format!(
                        "object {ino} was not reached: named /lost+found/{name}"
                    ));
                }
                return Ok(());
            }
        }
        Err(Error::Invalid("no free name in /lost+found"))
    }

    /// Rule 6.
    fn counts(&mut self, named: &BTreeMap<u64, Vec<u64>>) {
        let txn = self.fs.io.txn;
        let mut fixes = Vec::new();
        for (ino, inode) in self.state.inodes.iter_mut() {
            let mut want = *inode;
            let by = named.get(ino).map_or(&[][..], |v| &v[..]);
            match inode.kind {
                FileType::Dir => {
                    let entries = self
                        .state
                        .names
                        .get(ino)
                        .into_iter()
                        .flat_map(|c| c.values().flatten());
                    let (count, subdirs) = entries.fold((0u64, 0u32), |(n, d), e| {
                        (n + 1, d + u32::from(e.kind == FileType::Dir))
                    });
                    want.size = count;
                    want.nlink = 2 + subdirs;
                    want.parent = by.first().copied().unwrap_or(ROOT);
                }
                FileType::File => {
                    want.nlink = by.len() as u32;
                    if let Some(&first) = by.first().filter(|_| !by.contains(&inode.parent)) {
                        want.parent = first;
                    }
                    let end = self
                        .state
                        .data
                        .get(ino)
                        .and_then(|d| d.iter().next_back())
                        .map(|(at, raw)| at + value_len(raw).unwrap_or(0) as u64);
                    want.size = want.size.max(end.unwrap_or(0)).min(MAX_FILE);
                }
            }
            want.changed = want.changed.min(txn);
            if want != *inode {
                fixes.push(format!(
                    "object {ino}: size, links and parent set from its names"
                ));
                *inode = want;
            }
        }
        self.fixes.extend(fixes);
    }

    /// The items the repaired namespace is made of, raw and in key order.
    fn items(&self) -> Vec<(Key, Vec<u8>)> {
        let mut out: BTreeMap<Key, Vec<u8>> = BTreeMap::new();
        for (ino, inode) in &self.state.inodes {
            out.insert(Key::new(*ino, K_INODE, 0), inline_raw(&inode.encode()));
        }
        for (dir, chains) in &self.state.names {
            for (hash, chain) in chains {
                out.insert(
                    Key::new(*dir, K_DIRENT, *hash),
                    inline_raw(&encode_names(chain)),
                );
            }
        }
        for (ino, items) in &self.state.data {
            for (at, raw) in items {
                out.insert(Key::new(*ino, K_DATA, *at), raw.clone());
            }
        }
        for (ino, chains) in &self.state.attrs {
            for (hash, chain) in chains {
                out.insert(
                    Key::new(*ino, K_XATTR, *hash),
                    inline_raw(&encode_attrs(chain)),
                );
            }
        }
        out.into_iter().collect()
    }
}

/// Applies the rules to the live tree and commits what they changed. Returns one
/// line a change; none, and no commit, if the namespace already kept them.
pub fn repair<D: BlockDev>(fs: &mut Fs<D>) -> Result<Vec<String>, Error> {
    if !fs.writable() {
        return Err(Error::ReadOnly);
    }
    if !fs.repairable() {
        return Err(Error::Unsupported(
            "an unknown compat feature: repair refuses",
        ));
    }
    let root = fs
        .live_root()
        .filter(|_| fs.is_clean())
        .ok_or(Error::Invalid("an open transaction"))?;
    let mut fixes = Vec::new();
    let mut found = Vec::new();
    if !root.is_null() {
        salvage(fs, &root, &mut found, &mut fixes);
    }
    let lost_nodes = !fixes.is_empty();
    let (uuid, mask, next_oid) = (fs.uuid(), fs.hash_mask, fs.next_oid());
    let mut repair = Repair {
        fs,
        state: State::default(),
        fixes,
        uuid,
        mask,
        lost: None,
    };
    for (key, raw) in found.iter().cloned() {
        repair.item(key, raw);
    }
    if let Some(&max) = repair.state.inodes.keys().next_back() {
        repair.fs.bump_oid(max);
        if repair.fs.next_oid() != next_oid {
            repair.fix(format!("the object id counter raised past {max}"));
        }
    }
    repair.root();
    repair.dangling();
    let named = repair.walk()?;
    repair.counts(&named);
    let want = repair.items();
    let Repair { fs, fixes, .. } = repair;
    if want == found && !lost_nodes && fs.next_oid() == next_oid {
        return Ok(Vec::new());
    }
    match lost_nodes {
        true => fs.replace_live(want)?,
        false => {
            let want_keys: BTreeSet<Key> = want.iter().map(|(k, _)| *k).collect();
            for (key, _) in found.iter().filter(|(k, _)| !want_keys.contains(k)) {
                fs.delete(key)?;
            }
            let had: BTreeMap<Key, Vec<u8>> = found.into_iter().collect();
            for (key, raw) in want {
                if had.get(&key) != Some(&raw) {
                    fs.set(key, raw)?;
                }
            }
        }
    }
    fs.commit()?;
    Ok(fixes)
}
