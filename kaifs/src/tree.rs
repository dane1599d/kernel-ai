//! The copy-on-write B+tree (ADR 0004 §1-2): format 1's keys `(oid, kind, off)`,
//! values of at most [`MAX_VALUE`] bytes, and 4 KiB nodes that each carry a `fmt`
//! byte, so a node of an older layout is upgraded when it is next written.
//!
//! A node read from disk is `Clean`. Changing it makes it `Dirty` in memory and
//! reports its old block as replaced, and so up to the root (path copying).
//! Writing the tree out writes every dirty node to a new block, children first, so
//! each parent records its children's checksums: the tree is a Merkle tree.
//! Deletes merge or rebalance a node that falls under a quarter full, and the root
//! shrinks when it has one child, so the tree stays shallow and dense. A key added
//! at the right end of the tree keeps the full nodes full and starts new ones, as
//! keys written in order do (new objects' ids): only nodes on the tree's
//! rightmost path may then be under a quarter full.
//!
//! Node layout: a 16-byte header (`fmt`, `level`, item count, reserved), then
//! items. A leaf item is its 17-byte key, a 2-byte value length and the value; a
//! branch item is the lowest key its child may hold and the child's 64-byte
//! [`BlockPtr`].

use crate::format::{self, BlockPtr, KIND_NODE, PTR_BYTES};
use crate::{BLOCK, Block, Error};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// A tree key, ordered by `oid`, then `kind`, then `off` (ADR 0004 §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub oid: u64,
    pub kind: u8,
    pub off: u64,
}

impl Key {
    pub const MIN: Key = Key {
        oid: 0,
        kind: 0,
        off: 0,
    };

    pub fn new(oid: u64, kind: u8, off: u64) -> Self {
        Key { oid, kind, off }
    }

    /// The key just after this one, if there is one.
    pub fn next(&self) -> Option<Key> {
        match (
            self.off.checked_add(1),
            self.kind.checked_add(1),
            self.oid.checked_add(1),
        ) {
            (Some(off), _, _) => Some(Key::new(self.oid, self.kind, off)),
            (None, Some(kind), _) => Some(Key::new(self.oid, kind, 0)),
            (None, None, Some(oid)) => Some(Key::new(oid, 0, 0)),
            (None, None, None) => None,
        }
    }

    fn put(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.oid.to_le_bytes());
        out[8] = self.kind;
        out[9..17].copy_from_slice(&self.off.to_le_bytes());
    }

    fn get(bytes: &[u8]) -> Key {
        let u64_at = |at: usize| {
            let mut word = [0; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        Key {
            oid: u64_at(0),
            kind: bytes[8],
            off: u64_at(9),
        }
    }
}

/// The node layout this build writes.
pub const FMT: u8 = 1;
const HEADER: usize = 16;
pub const KEY_BYTES: usize = 17;
const ITEM_HEADER: usize = KEY_BYTES + 2;
const BRANCH_ENTRY: usize = KEY_BYTES + PTR_BYTES;
/// Children of a branch node.
pub const MAX_BRANCH: usize = (BLOCK - HEADER) / BRANCH_ENTRY;
const LEAF_SPACE: usize = BLOCK - HEADER;
/// The largest value an item holds inline.
pub const MAX_VALUE: usize = 1024;

/// What the tree needs of the filesystem below it.
pub trait Store {
    /// Reads, verifies and parses the node `ptr` names.
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error>;
    /// The block `ptr` names is replaced in this transaction.
    fn replaced(&mut self, ptr: BlockPtr);
    /// Writes a node to a new block and returns its pointer.
    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Child {
    Clean(BlockPtr),
    Dirty(Box<Node>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Leaf(Vec<(Key, Vec<u8>)>),
    /// Children by the lowest key each may hold, and the node's level (1 or more).
    Branch(u8, Vec<(Key, Child)>),
}

fn leaf_bytes(items: &[(Key, Vec<u8>)]) -> usize {
    items.iter().map(|(_, v)| ITEM_HEADER + v.len()).sum()
}

/// Where to cut `items` (more than a leaf holds, at least two items) so the two
/// halves are as even as can be: the smaller then holds at least a third of a
/// leaf, since no item is over a quarter of one.
fn balanced_cut(items: &[(Key, Vec<u8>)]) -> usize {
    let total = leaf_bytes(items);
    let (mut best, mut best_gap, mut prefix) = (1, usize::MAX, 0);
    for (i, (_, v)) in items.iter().enumerate().take(items.len() - 1) {
        prefix += ITEM_HEADER + v.len();
        let gap = prefix.abs_diff(total - prefix);
        if gap < best_gap {
            (best, best_gap) = (i + 1, gap);
        }
    }
    best
}

impl Node {
    pub fn level(&self) -> u8 {
        match self {
            Node::Leaf(_) => 0,
            Node::Branch(level, _) => *level,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Node::Leaf(items) => items.len(),
            Node::Branch(_, children) => children.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn first_key(&self) -> Option<Key> {
        match self {
            Node::Leaf(items) => items.first().map(|(k, _)| *k),
            Node::Branch(_, children) => children.first().map(|(k, _)| *k),
        }
    }

    /// Under a quarter full, so a delete merges or rebalances it.
    pub fn underfull(&self) -> bool {
        match self {
            Node::Leaf(items) => leaf_bytes(items) < LEAF_SPACE / 4,
            Node::Branch(_, children) => children.len() < MAX_BRANCH / 4,
        }
    }

    /// The node as a block. Every child must be clean.
    pub fn encode(&self) -> Block {
        let mut block = [0; BLOCK];
        block[0] = FMT;
        block[1] = self.level();
        block[2..4].copy_from_slice(&(self.len() as u16).to_le_bytes());
        let mut at = HEADER;
        match self {
            Node::Leaf(items) => {
                for (key, value) in items {
                    key.put(&mut block[at..]);
                    block[at + KEY_BYTES..at + ITEM_HEADER]
                        .copy_from_slice(&(value.len() as u16).to_le_bytes());
                    block[at + ITEM_HEADER..at + ITEM_HEADER + value.len()].copy_from_slice(value);
                    at += ITEM_HEADER + value.len();
                }
            }
            Node::Branch(_, children) => {
                for (key, child) in children {
                    let Child::Clean(ptr) = child else {
                        unreachable!("a branch is encoded after its children are written");
                    };
                    key.put(&mut block[at..]);
                    block[at + KEY_BYTES..at + BRANCH_ENTRY].copy_from_slice(&ptr.encode());
                    at += BRANCH_ENTRY;
                }
            }
        }
        block
    }

    /// Parses a node block whose checksum has verified, checking every rule a
    /// later step relies on: the layout, the counts, keys in strict order, value
    /// lengths, and children one level down.
    pub fn decode(block: &Block) -> Result<Self, Error> {
        if block[0] != FMT {
            return Err(Error::Corrupt("node format"));
        }
        let level = block[1];
        let count = u16::from_le_bytes([block[2], block[3]]) as usize;
        let mut at = HEADER;
        if level == 0 {
            let mut items: Vec<(Key, Vec<u8>)> =
                Vec::with_capacity(count.min(LEAF_SPACE / ITEM_HEADER));
            for _ in 0..count {
                if at + ITEM_HEADER > BLOCK {
                    return Err(Error::Corrupt("leaf items past the block"));
                }
                let key = Key::get(&block[at..]);
                let len =
                    u16::from_le_bytes([block[at + KEY_BYTES], block[at + KEY_BYTES + 1]]) as usize;
                if len > MAX_VALUE || at + ITEM_HEADER + len > BLOCK {
                    return Err(Error::Corrupt("leaf value length"));
                }
                if items.last().is_some_and(|(prev, _)| *prev >= key) {
                    return Err(Error::Corrupt("leaf keys out of order"));
                }
                items.push((
                    key,
                    block[at + ITEM_HEADER..at + ITEM_HEADER + len].to_vec(),
                ));
                at += ITEM_HEADER + len;
            }
            return Ok(Node::Leaf(items));
        }
        if level > format::MAX_LEVEL || count == 0 || count > MAX_BRANCH {
            return Err(Error::Corrupt("branch level or count"));
        }
        let mut children: Vec<(Key, Child)> = Vec::with_capacity(count);
        for _ in 0..count {
            let key = Key::get(&block[at..]);
            let ptr = BlockPtr::decode_at(&block[..], at + KEY_BYTES)?;
            if ptr.kind != KIND_NODE || ptr.level + 1 != level {
                return Err(Error::Corrupt("branch child kind or level"));
            }
            if children.last().is_some_and(|(prev, _)| *prev >= key) {
                return Err(Error::Corrupt("branch keys out of order"));
            }
            children.push((key, Child::Clean(ptr)));
            at += BRANCH_ENTRY;
        }
        Ok(Node::Branch(level, children))
    }
}

/// Makes `child` dirty, loading it and reporting its old block as replaced.
fn dirty<'a>(child: &'a mut Child, store: &mut dyn Store) -> Result<&'a mut Node, Error> {
    if let Child::Clean(ptr) = child {
        let ptr = *ptr;
        let node = store.load(&ptr)?;
        store.replaced(ptr);
        *child = Child::Dirty(Box::new(node));
    }
    match child {
        Child::Dirty(node) => Ok(node),
        Child::Clean(_) => unreachable!("made dirty above"),
    }
}

/// The child of a branch that holds `key`: the last whose lower bound is at most
/// `key`, or the first.
fn slot(children: &[(Key, Child)], key: &Key) -> usize {
    children
        .partition_point(|(lower, _)| lower <= key)
        .saturating_sub(1)
}

type Split = Option<(Key, Node)>;

/// `rightmost`: the node is on the tree's rightmost path, where an item added at
/// the end is appended to the tree as a whole.
fn insert(
    node: &mut Node,
    key: Key,
    value: Vec<u8>,
    store: &mut dyn Store,
    rightmost: bool,
) -> Result<(Option<Vec<u8>>, Split), Error> {
    match node {
        Node::Leaf(items) => {
            let (old, at) = match items.binary_search_by_key(&key, |(k, _)| *k) {
                Ok(i) => (Some(core::mem::replace(&mut items[i].1, value)), i),
                Err(i) => {
                    items.insert(i, (key, value));
                    (None, i)
                }
            };
            if leaf_bytes(items) <= LEAF_SPACE {
                return Ok((old, None));
            }
            // An append to the tree keeps the full leaf full and starts the next:
            // keys written in order, such as new objects' ids, fill leaves.
            let cut = match rightmost && old.is_none() && at == items.len() - 1 {
                true => at,
                false => balanced_cut(items),
            };
            let right = items.split_off(cut);
            Ok((old, Some((right[0].0, Node::Leaf(right)))))
        }
        Node::Branch(level, children) => {
            let i = slot(children, &key);
            if key < children[i].0 {
                children[i].0 = key;
            }
            let last = i == children.len() - 1;
            let len = value.len();
            let child = dirty(&mut children[i].1, store)?;
            let (old, split) = insert(child, key, value, store, rightmost && last)?;
            let shrank = old.as_ref().is_some_and(|o| o.len() > len);
            match split {
                Some((lower, right)) => {
                    children.insert(i + 1, (lower, Child::Dirty(Box::new(right))))
                }
                // A shorter value can leave a leaf under a quarter full.
                None if shrank && child.underfull() && children.len() > 1 => {
                    rebalance(children, i, store)?
                }
                None => {}
            }
            if children.len() <= MAX_BRANCH {
                return Ok((old, None));
            }
            let cut = match rightmost && last {
                true => children.len() - 1,
                false => children.len() / 2,
            };
            let right = children.split_off(cut);
            Ok((old, Some((right[0].0, Node::Branch(*level, right)))))
        }
    }
}

fn remove(node: &mut Node, key: &Key, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
    match node {
        Node::Leaf(items) => Ok(items
            .binary_search_by_key(key, |(k, _)| *k)
            .ok()
            .map(|i| items.remove(i).1)),
        Node::Branch(_, children) => {
            let i = slot(children, key);
            let child = dirty(&mut children[i].1, store)?;
            let old = remove(child, key, store)?;
            if child.underfull() && children.len() > 1 {
                rebalance(children, i, store)?;
            }
            Ok(old)
        }
    }
}

/// Child `i` fell under a quarter full: merge it with a neighbour if both fit in
/// one node, else even the two out.
fn rebalance(
    children: &mut Vec<(Key, Child)>,
    i: usize,
    store: &mut dyn Store,
) -> Result<(), Error> {
    let a = if i + 1 < children.len() { i } else { i - 1 };
    let (left, right) = children.split_at_mut(a + 1);
    let left_node = dirty(&mut left[a].1, store)?;
    let right_node = dirty(&mut right[0].1, store)?;
    match (left_node, right_node) {
        (Node::Leaf(l), Node::Leaf(r)) => {
            l.append(r);
            if leaf_bytes(l) > LEAF_SPACE {
                *r = l.split_off(balanced_cut(l));
            }
        }
        (Node::Branch(_, l), Node::Branch(_, r)) => {
            if l.len() + r.len() <= MAX_BRANCH {
                l.append(r);
            } else {
                let half = (l.len() + r.len()) / 2;
                while l.len() < half {
                    l.push(r.remove(0));
                }
                while l.len() > half {
                    let last = l.pop().unwrap_or_else(|| unreachable!("over half"));
                    r.insert(0, last);
                }
            }
        }
        _ => return Err(Error::Corrupt("siblings of different levels")),
    }
    let right_empty = match &children[a + 1].1 {
        Child::Dirty(node) => node.is_empty(),
        Child::Clean(_) => false,
    };
    if right_empty {
        children.remove(a + 1);
    } else if let Child::Dirty(node) = &children[a + 1].1
        && let Some(first) = node.first_key()
    {
        children[a + 1].0 = first;
    }
    Ok(())
}

/// A tree: no root when empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub root: Option<Child>,
}

impl Tree {
    /// The tree whose root `ptr` names, or an empty tree for the null pointer.
    pub fn open(ptr: BlockPtr) -> Self {
        Tree {
            root: (!ptr.is_null()).then_some(Child::Clean(ptr)),
        }
    }

    pub fn get(&self, key: &Key, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
        let Some(root) = &self.root else {
            return Ok(None);
        };
        let mut owned;
        let mut node: &Node = match root {
            Child::Dirty(node) => node,
            Child::Clean(ptr) => {
                owned = store.load(ptr)?;
                &owned
            }
        };
        loop {
            match node {
                Node::Leaf(items) => {
                    return Ok(items
                        .binary_search_by_key(key, |(k, _)| *k)
                        .ok()
                        .map(|i| items[i].1.clone()));
                }
                Node::Branch(_, children) => match &children[slot(children, key)].1 {
                    Child::Dirty(child) => node = child,
                    Child::Clean(ptr) => {
                        owned = store.load(ptr)?;
                        node = &owned;
                    }
                },
            }
        }
    }

    /// Sets `key` to `value`; returns the old value.
    pub fn insert(
        &mut self,
        key: Key,
        value: Vec<u8>,
        store: &mut dyn Store,
    ) -> Result<Option<Vec<u8>>, Error> {
        assert!(value.len() <= MAX_VALUE, "tree values are at most 1 KiB");
        let Some(root) = self.root.as_mut() else {
            self.root = Some(Child::Dirty(Box::new(Node::Leaf(alloc::vec![(
                key, value
            )]))));
            return Ok(None);
        };
        let (old, split) = insert(dirty(root, store)?, key, value, store, true)?;
        if let Some((lower, right)) = split {
            let Some(Child::Dirty(left)) = self.root.take() else {
                unreachable!("the root was made dirty");
            };
            let level = left.level() + 1;
            let first = left.first_key().map_or(key, |k| k.min(key));
            let children = alloc::vec![
                (first, Child::Dirty(left)),
                (lower, Child::Dirty(Box::new(right)))
            ];
            self.root = Some(Child::Dirty(Box::new(Node::Branch(level, children))));
        }
        Ok(old)
    }

    /// Removes `key`; returns its value. A key that is absent copies nothing.
    pub fn remove(&mut self, key: &Key, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
        if self.get(key, store)?.is_none() {
            return Ok(None);
        }
        let Some(root) = self.root.as_mut() else {
            return Ok(None);
        };
        let old = remove(dirty(root, store)?, key, store)?;
        // A root with one child gives way to it; an empty leaf root empties the tree.
        loop {
            match self.root.as_mut() {
                Some(Child::Dirty(node)) if node.is_empty() => self.root = None,
                Some(Child::Dirty(node)) => match &mut **node {
                    Node::Branch(_, children) if children.len() == 1 => {
                        let (_, only) = children.pop().unwrap_or_else(|| unreachable!("one child"));
                        self.root = Some(only);
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        Ok(old)
    }

    /// Calls `visit` on every item from `from` on, in key order, until it returns
    /// false.
    pub fn scan(
        &self,
        from: &Key,
        store: &mut dyn Store,
        visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
    ) -> Result<(), Error> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        scan(root, Bounds::ALL, from, store, visit).map(|_| ())
    }

    /// Writes every dirty node; returns the root's pointer, or null if empty.
    pub fn write_out(&mut self, store: &mut dyn Store) -> Result<BlockPtr, Error> {
        match self.root.as_mut() {
            None => Ok(BlockPtr::NULL),
            Some(child) => write_child(child, store),
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.root, Some(Child::Dirty(_)))
    }

    /// Calls `visit` on every item in a leaf born after generation `since` (a
    /// dirty node is born now), reading only subtrees born after it: since every
    /// change rewrites its leaf and the leaf's parents, the rest has not changed.
    pub fn scan_born_after(
        &self,
        since: u64,
        store: &mut dyn Store,
        visit: &mut dyn FnMut(&Key, &[u8]),
    ) -> Result<(), Error> {
        match &self.root {
            None => Ok(()),
            Some(root) => born_after(root, Bounds::ALL, since, store, visit),
        }
    }
}

/// The keys a node may hold, as its parents route them: from `lo` on, and
/// below `hi` if there is one.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    lo: Key,
    hi: Option<Key>,
}

impl Bounds {
    const ALL: Bounds = Bounds {
        lo: Key::MIN,
        hi: None,
    };

    /// Child `i`'s: from its key on (the first child takes every key below its
    /// key too, as a lookup routes them), to the next child's key.
    fn child(self, children: &[(Key, Child)], i: usize) -> Bounds {
        let lo = match i {
            0 => self.lo,
            _ => self.lo.max(children[i].0),
        };
        let hi = match (children.get(i + 1).map(|(k, _)| *k), self.hi) {
            (Some(k), Some(h)) => Some(k.min(h)),
            (k, h) => k.or(h),
        };
        Bounds { lo, hi }
    }

    fn holds(self, key: &Key) -> bool {
        *key >= self.lo && self.hi.is_none_or(|h| *key < h)
    }
}

/// Loads a node a walk reaches, which must hold together where it lies: a leaf
/// holds at least one item, each inside the bounds its parents give it. The
/// bounds of a tree's nodes never overlap, so a node a hostile disk names twice
/// fails at its second visit, and no walk reads more nodes than the disk holds.
fn load_within(ptr: &BlockPtr, bounds: Bounds, store: &mut dyn Store) -> Result<Node, Error> {
    let node = store.load(ptr)?;
    if let Node::Leaf(items) = &node {
        let (Some((first, _)), Some((last, _))) = (items.first(), items.last()) else {
            return Err(Error::Corrupt("an empty leaf"));
        };
        if !bounds.holds(first) || !bounds.holds(last) {
            return Err(Error::Corrupt("keys outside their parent's bounds"));
        }
    }
    Ok(node)
}

fn born_after(
    child: &Child,
    bounds: Bounds,
    since: u64,
    store: &mut dyn Store,
    visit: &mut dyn FnMut(&Key, &[u8]),
) -> Result<(), Error> {
    let owned;
    let node = match child {
        Child::Clean(ptr) if ptr.birth <= since => return Ok(()),
        Child::Clean(ptr) => {
            owned = load_within(ptr, bounds, store)?;
            &owned
        }
        Child::Dirty(node) => &**node,
    };
    match node {
        Node::Leaf(items) => items.iter().for_each(|(k, v)| visit(k, v)),
        Node::Branch(_, children) => {
            for (i, (_, child)) in children.iter().enumerate() {
                born_after(child, bounds.child(children, i), since, store, visit)?;
            }
        }
    }
    Ok(())
}

/// Whether to go on.
fn scan(
    child: &Child,
    bounds: Bounds,
    from: &Key,
    store: &mut dyn Store,
    visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
) -> Result<bool, Error> {
    let owned;
    let node = match child {
        Child::Dirty(node) => &**node,
        Child::Clean(ptr) => {
            owned = load_within(ptr, bounds, store)?;
            &owned
        }
    };
    match node {
        Node::Leaf(items) => {
            let start = items.partition_point(|(k, _)| k < from);
            for (key, value) in &items[start..] {
                if !visit(key, value) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Node::Branch(_, children) => {
            for i in slot(children, from)..children.len() {
                let bounds = bounds.child(children, i);
                if !scan(&children[i].1, bounds, from, store, visit)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

fn write_child(child: &mut Child, store: &mut dyn Store) -> Result<BlockPtr, Error> {
    if let Child::Dirty(node) = child {
        if let Node::Branch(_, children) = &mut **node {
            for (_, c) in children.iter_mut() {
                write_child(c, store)?;
            }
        }
        let ptr = store.store(&node.encode(), node.level())?;
        *child = Child::Clean(ptr);
    }
    match child {
        Child::Clean(ptr) => Ok(*ptr),
        Child::Dirty(_) => unreachable!("written above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csum::CsumKind;
    use alloc::collections::{BTreeMap, BTreeSet};

    /// Nodes in memory: every block written once, and replaced at most once.
    #[derive(Default)]
    struct MemStore {
        blocks: BTreeMap<u64, Block>,
        next: u64,
        replaced: BTreeSet<u64>,
    }

    impl Store for MemStore {
        fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
            let block = self
                .blocks
                .get(&ptr.addr)
                .ok_or(Error::Corrupt("missing"))?;
            assert_eq!(CsumKind::Seahash64.sum(block), ptr.csum);
            assert!(!self.replaced.contains(&ptr.addr), "read a replaced block");
            Node::decode(block)
        }

        fn replaced(&mut self, ptr: BlockPtr) {
            assert!(
                self.replaced.insert(ptr.addr),
                "block {} replaced twice",
                ptr.addr
            );
        }

        fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
            self.next += 1;
            self.blocks.insert(self.next, *block);
            Ok(BlockPtr {
                addr: self.next,
                birth: 1,
                blocks: 1,
                kind: KIND_NODE,
                level,
                flags: 0,
                csum: CsumKind::Seahash64.sum(block),
            })
        }
    }

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n.max(1)
        }
    }

    /// Checks the tree under `ptr`: order, bounds, levels and fill (`edge`: the
    /// node is on the tree's rightmost path, where it may be under a quarter full);
    /// returns the blocks it reaches.
    fn shape(
        store: &mut MemStore,
        ptr: &BlockPtr,
        lower: Key,
        upper: Option<Key>,
        edge: bool,
        reached: &mut BTreeSet<u64>,
    ) -> usize {
        reached.insert(ptr.addr);
        let node = store.load(ptr).unwrap();
        assert!(
            edge || !node.underfull(),
            "an underfull node off the rightmost path"
        );
        match node {
            Node::Leaf(items) => {
                assert!(
                    items
                        .iter()
                        .all(|(k, _)| *k >= lower && upper.is_none_or(|u| *k < u))
                );
                items.len()
            }
            Node::Branch(level, children) => {
                let mut count = 0;
                for (i, (key, child)) in children.iter().enumerate() {
                    let Child::Clean(p) = child else {
                        unreachable!()
                    };
                    assert_eq!(p.level + 1, level);
                    let next = children.get(i + 1).map(|(k, _)| *k).or(upper);
                    let edge = edge && i == children.len() - 1;
                    count += shape(store, p, (*key).max(lower), next, edge, reached);
                }
                count
            }
        }
    }

    fn value(rng: &mut Rng) -> Vec<u8> {
        let len = [1, 8, 40, 200, 1024][rng.below(5) as usize];
        (0..len).map(|_| rng.below(256) as u8).collect()
    }

    #[test]
    fn random_operations_agree_with_a_map_and_keep_the_tree_sound() {
        let mut store = MemStore::default();
        let mut tree = Tree::default();
        let mut model: BTreeMap<Key, Vec<u8>> = BTreeMap::new();
        let mut rng = Rng(0xC0FFEE);
        for step in 0..30_000u64 {
            let key = Key::new(rng.below(600), rng.below(3) as u8, rng.below(8));
            match rng.below(10) {
                0..=5 => {
                    let v = value(&mut rng);
                    assert_eq!(
                        tree.insert(key, v.clone(), &mut store).unwrap(),
                        model.insert(key, v)
                    );
                }
                6..=8 => assert_eq!(tree.remove(&key, &mut store).unwrap(), model.remove(&key)),
                _ => assert_eq!(
                    tree.get(&key, &mut store).unwrap(),
                    model.get(&key).cloned()
                ),
            }
            if step % 997 == 0 {
                let ptr = tree.write_out(&mut store).unwrap();
                let mut reached = BTreeSet::new();
                let count = match ptr.is_null() {
                    true => 0,
                    false => shape(&mut store, &ptr, Key::MIN, None, true, &mut reached),
                };
                assert_eq!(count, model.len(), "step {step}");
                assert!(
                    reached.is_disjoint(&store.replaced),
                    "step {step}: a reached block was replaced"
                );
                let live = store.blocks.len() - store.replaced.len();
                assert_eq!(
                    live,
                    reached.len(),
                    "step {step}: every unreplaced block is reached"
                );
                let mut seen = Vec::new();
                tree.scan(&Key::MIN, &mut store, &mut |k, v| {
                    seen.push((*k, v.to_vec()));
                    true
                })
                .unwrap();
                assert_eq!(
                    seen,
                    model
                        .iter()
                        .map(|(k, v)| (*k, v.clone()))
                        .collect::<Vec<_>>()
                );
            }
        }
        for key in model.keys().copied().collect::<Vec<_>>() {
            tree.remove(&key, &mut store).unwrap();
        }
        assert_eq!(tree.root, None, "removing everything empties the tree");
    }

    #[test]
    fn a_scan_starts_at_its_key_and_stops_when_asked() {
        let mut store = MemStore::default();
        let mut tree = Tree::default();
        for i in 0..5_000u64 {
            tree.insert(
                Key::new(i / 10, 1, i % 10),
                alloc::vec![i as u8; 30],
                &mut store,
            )
            .unwrap();
        }
        tree.write_out(&mut store).unwrap();
        let mut seen = Vec::new();
        tree.scan(&Key::new(321, 0, 0), &mut store, &mut |k, _| {
            seen.push(*k);
            seen.len() < 15
        })
        .unwrap();
        assert_eq!(seen.len(), 15);
        assert_eq!(seen[0], Key::new(321, 1, 0));
        assert_eq!(seen[14], Key::new(322, 1, 4));
    }

    #[test]
    fn a_node_round_trips() {
        let leaf = Node::Leaf(alloc::vec![
            (Key::new(1, 2, 3), alloc::vec![9; 10]),
            (Key::new(1, 2, 4), alloc::vec![])
        ]);
        assert_eq!(Node::decode(&leaf.encode()), Ok(leaf));
        let ptr = BlockPtr {
            addr: 77,
            birth: 3,
            blocks: 1,
            kind: KIND_NODE,
            level: 0,
            flags: 0,
            csum: [1; 32],
        };
        let branch = Node::Branch(
            1,
            alloc::vec![
                (Key::MIN, Child::Clean(ptr)),
                (Key::new(5, 0, 0), Child::Clean(ptr))
            ],
        );
        assert_eq!(Node::decode(&branch.encode()), Ok(branch));
    }
}
