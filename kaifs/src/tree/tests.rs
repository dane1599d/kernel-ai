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
