//! Audit item 4 (`tree.rs:349`): an append at the right edge can put the new
//! key alone in a new leaf, alone in a new branch. Removing that key left the
//! empty leaf in its one-child branch, the parent then evened the branch out
//! with its neighbour, and the written tree held a leaf of no items, which
//! every scan to the end refused as corrupt. An emptied child now leaves its
//! branch whatever its size.

use kaifs::format::{BlockPtr, KIND_NODE};
use kaifs::tree::{Child, Key, Node, Store, Tree};
use kaifs::{Block, CsumKind, Error};
use std::collections::HashMap;

const KIND: CsumKind = CsumKind::Seahash64;

/// Nodes in memory, as blocks, as a disk would hold them.
#[derive(Default)]
struct Mem {
    blocks: HashMap<u64, Block>,
    next: u64,
}

impl Store for Mem {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        let block = self
            .blocks
            .get(&ptr.addr)
            .ok_or(Error::Corrupt("no block"))?;
        Node::decode(block)
    }

    fn replaced(&mut self, _: BlockPtr) {}

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
            csum: KIND.sum(block),
        })
    }
}

/// How many items the tree's last leaf holds, with every node in memory.
fn last_leaf_len(tree: &Tree) -> usize {
    let mut node = match &tree.root {
        Some(Child::Dirty(node)) => &**node,
        _ => return 0,
    };
    loop {
        match node {
            Node::Leaf(items) => return items.len(),
            Node::Branch(_, children) => match children.last() {
                Some((_, Child::Dirty(child))) => node = child,
                _ => return 0,
            },
        }
    }
}

fn key(n: u64) -> Key {
    Key::new(n, 1, 0)
}

#[test]
fn removing_the_key_an_append_split_off_leaves_no_empty_leaf() {
    let mut store = Mem::default();
    let mut tree = Tree::default();
    let mut tried = 0;
    for n in 0..20_000u64 {
        tree.insert(key(n), vec![7; 8], &mut store).unwrap();
        // The append's key alone in the last leaf: remove it, write the tree
        // out, and scan it all from disk.
        if n == 0 || last_leaf_len(&tree) != 1 {
            continue;
        }
        tried += 1;
        let mut cut = tree.clone();
        assert!(cut.remove(&key(n), &mut store).unwrap().is_some());
        let root = cut.write_out(&mut store).unwrap();
        let mut seen = 0u64;
        Tree::open(root)
            .scan(&Key::MIN, &mut store, &mut |_, _| {
                seen += 1;
                true
            })
            .unwrap_or_else(|e| panic!("after {n} keys: {e:?}"));
        assert_eq!(seen, n, "after {n} keys");
    }
    assert!(tried > 100, "only {tried} right-edge splits");
}
