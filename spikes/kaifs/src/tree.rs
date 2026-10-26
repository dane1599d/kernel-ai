//! A copy-on-write B+tree of u64 keys and values of at most 1 KiB, in 4 KiB nodes.
//!
//! A node read from disk is `Clean`; changing it makes it `Dirty` in memory and
//! reports the old block as freed, and so up to the root (path copying). Writing
//! the tree out writes every dirty node to a new block, children first, so each
//! parent records its children's checksums.
//!
//! Deletes drop empty nodes but do not rebalance: crash consistency does not
//! depend on balance, and the spike's figures are taken on trees built by inserts.

use crate::format::{BlockPtr, KIND_NODE};
use crate::fs::Error;
use crate::{BLOCK, Block, MAX_VALUE};
use alloc::boxed::Box;
use alloc::vec::Vec;

const HEADER: usize = 8;
const FMT: u8 = 1;
const BRANCH_ENTRY: usize = 8 + 64;
/// Children of a branch node.
pub const MAX_BRANCH: usize = (BLOCK - HEADER) / BRANCH_ENTRY;
const LEAF_SPACE: usize = BLOCK - HEADER;

/// What the tree needs of the filesystem below it.
pub trait Store {
    /// Reads, verifies and parses the node `ptr` names.
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error>;
    /// The block `ptr` names is replaced in this transaction.
    fn freed(&mut self, ptr: BlockPtr);
    /// Writes a node to a new block and returns its pointer.
    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error>;
}

#[derive(Clone, Debug)]
pub enum Child {
    Clean(BlockPtr),
    Dirty(Box<Node>),
}

#[derive(Clone, Debug)]
pub enum Node {
    Leaf(Vec<(u64, Vec<u8>)>),
    /// Children by the lowest key each may hold, and the node's level (at least 1).
    Branch(u8, Vec<(u64, Child)>),
}

/// A tree: no root when empty.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub root: Option<Child>,
}

impl Node {
    pub fn level(&self) -> u8 {
        match self {
            Node::Leaf(_) => 0,
            Node::Branch(level, _) => *level,
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Node::Leaf(items) => items.is_empty(),
            Node::Branch(_, children) => children.is_empty(),
        }
    }

    fn first_key(&self) -> u64 {
        match self {
            Node::Leaf(items) => items.first().map_or(0, |(k, _)| *k),
            Node::Branch(_, children) => children.first().map_or(0, |(k, _)| *k),
        }
    }

    fn leaf_bytes(items: &[(u64, Vec<u8>)]) -> usize {
        items.iter().map(|(_, v)| 10 + v.len()).sum()
    }

    /// The node as a block; every child must be clean.
    pub fn encode(&self) -> Block {
        let mut block = [0; BLOCK];
        block[0] = FMT;
        block[1] = self.level();
        let mut at = HEADER;
        match self {
            Node::Leaf(items) => {
                block[2..4].copy_from_slice(&(items.len() as u16).to_le_bytes());
                for (key, value) in items {
                    block[at..at + 8].copy_from_slice(&key.to_le_bytes());
                    block[at + 8..at + 10].copy_from_slice(&(value.len() as u16).to_le_bytes());
                    block[at + 10..at + 10 + value.len()].copy_from_slice(value);
                    at += 10 + value.len();
                }
            }
            Node::Branch(_, children) => {
                block[2..4].copy_from_slice(&(children.len() as u16).to_le_bytes());
                for (key, child) in children {
                    let Child::Clean(ptr) = child else {
                        unreachable!("a branch is encoded after its children are written");
                    };
                    block[at..at + 8].copy_from_slice(&key.to_le_bytes());
                    block[at + 8..at + BRANCH_ENTRY]
                        .copy_from_slice(zerocopy::IntoBytes::as_bytes(ptr));
                    at += BRANCH_ENTRY;
                }
            }
        }
        block
    }

    /// Parses a node block whose checksum already verified.
    pub fn decode(block: &Block) -> Result<Self, Error> {
        let bad = Error::Corrupt("malformed node");
        if block[0] != FMT {
            return Err(bad);
        }
        let level = block[1];
        let count = u16::from_le_bytes([block[2], block[3]]) as usize;
        let mut at = HEADER;
        let u64_at = |at: usize| u64::from_le_bytes(block[at..at + 8].try_into().unwrap());
        if level == 0 {
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                if at + 10 > BLOCK {
                    return Err(bad);
                }
                let len = u16::from_le_bytes([block[at + 8], block[at + 9]]) as usize;
                if len > MAX_VALUE || at + 10 + len > BLOCK {
                    return Err(bad);
                }
                items.push((u64_at(at), block[at + 10..at + 10 + len].to_vec()));
                at += 10 + len;
            }
            Ok(Node::Leaf(items))
        } else {
            if count > MAX_BRANCH || count == 0 {
                return Err(bad);
            }
            let mut children = Vec::with_capacity(count);
            for _ in 0..count {
                let (ptr, _) =
                    <BlockPtr as zerocopy::FromBytes>::read_from_prefix(&block[at + 8..])
                        .map_err(|_| bad)?;
                children.push((u64_at(at), Child::Clean(ptr)));
                at += BRANCH_ENTRY;
            }
            Ok(Node::Branch(level, children))
        }
    }
}

/// Makes `child` dirty, loading it and reporting its old block as freed.
fn dirty<'a>(child: &'a mut Child, store: &mut dyn Store) -> Result<&'a mut Node, Error> {
    if let Child::Clean(ptr) = child {
        let ptr = *ptr;
        let node = store.load(&ptr)?;
        store.freed(ptr);
        *child = Child::Dirty(Box::new(node));
    }
    match child {
        Child::Dirty(node) => Ok(node),
        Child::Clean(_) => unreachable!(),
    }
}

/// The child of a branch that holds `key`: the last whose lower bound is at most
/// `key`, or the first.
fn slot(children: &[(u64, Child)], key: u64) -> usize {
    children
        .partition_point(|(lower, _)| *lower <= key)
        .saturating_sub(1)
}

type Split = Option<(u64, Node)>;

fn insert(
    node: &mut Node,
    key: u64,
    value: Vec<u8>,
    store: &mut dyn Store,
) -> Result<(Option<Vec<u8>>, Split), Error> {
    match node {
        Node::Leaf(items) => {
            let old = match items.binary_search_by_key(&key, |(k, _)| *k) {
                Ok(i) => Some(core::mem::replace(&mut items[i].1, value)),
                Err(i) => {
                    items.insert(i, (key, value));
                    None
                }
            };
            if Node::leaf_bytes(items) <= LEAF_SPACE {
                return Ok((old, None));
            }
            // Split at the middle by bytes.
            let half = Node::leaf_bytes(items) / 2;
            let (mut acc, mut at) = (0, 0);
            while at < items.len() - 1 && acc < half {
                acc += 10 + items[at].1.len();
                at += 1;
            }
            let right = items.split_off(at.max(1));
            Ok((old, Some((right[0].0, Node::Leaf(right)))))
        }
        Node::Branch(level, children) => {
            let i = slot(children, key);
            if key < children[i].0 {
                children[i].0 = key;
            }
            let (old, split) = insert(dirty(&mut children[i].1, store)?, key, value, store)?;
            if let Some((lower, right)) = split {
                children.insert(i + 1, (lower, Child::Dirty(Box::new(right))));
            }
            if children.len() <= MAX_BRANCH {
                return Ok((old, None));
            }
            let right = children.split_off(children.len() / 2);
            let lower = right[0].0;
            Ok((old, Some((lower, Node::Branch(*level, right)))))
        }
    }
}

fn remove(node: &mut Node, key: u64, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
    match node {
        Node::Leaf(items) => Ok(items
            .binary_search_by_key(&key, |(k, _)| *k)
            .ok()
            .map(|i| items.remove(i).1)),
        Node::Branch(_, children) => {
            let i = slot(children, key);
            let child = dirty(&mut children[i].1, store)?;
            let old = remove(child, key, store)?;
            if child.is_empty() {
                children.remove(i);
            }
            Ok(old)
        }
    }
}

impl Tree {
    pub fn get(&self, key: u64, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
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
                        .binary_search_by_key(&key, |(k, _)| *k)
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
        key: u64,
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
        let node = dirty(root, store)?;
        let (old, split) = insert(node, key, value, store)?;
        if let Some((lower, right)) = split {
            let Some(Child::Dirty(left)) = self.root.take() else {
                unreachable!()
            };
            let level = left.level() + 1;
            let first = left.first_key().min(key);
            let children = alloc::vec![
                (first, Child::Dirty(left)),
                (lower, Child::Dirty(Box::new(right)))
            ];
            self.root = Some(Child::Dirty(Box::new(Node::Branch(level, children))));
        }
        Ok(old)
    }

    /// Removes `key`; returns its value. Only call it for a key that is present,
    /// or the path to where it would be is copied for nothing.
    pub fn remove(&mut self, key: u64, store: &mut dyn Store) -> Result<Option<Vec<u8>>, Error> {
        let Some(root) = self.root.as_mut() else {
            return Ok(None);
        };
        let node = dirty(root, store)?;
        let old = remove(node, key, store)?;
        loop {
            match self.root.as_mut() {
                Some(Child::Dirty(node)) if node.is_empty() => self.root = None,
                Some(Child::Dirty(node)) => match &mut **node {
                    Node::Branch(_, children) if children.len() == 1 => {
                        let (_, only) = children.pop().unwrap();
                        self.root = Some(only);
                        if let Some(Child::Clean(_)) = self.root {
                            break;
                        }
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        Ok(old)
    }

    /// Writes every dirty node; returns the root's pointer, or None if empty.
    pub fn write_out(&mut self, store: &mut dyn Store) -> Result<Option<BlockPtr>, Error> {
        match self.root.as_mut() {
            None => Ok(None),
            Some(child) => write_child(child, store).map(Some),
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.root, Some(Child::Dirty(_)))
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
    let Child::Clean(ptr) = child else {
        unreachable!()
    };
    debug_assert_eq!(ptr.kind, KIND_NODE);
    Ok(*ptr)
}
