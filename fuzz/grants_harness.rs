//! The `kai_grants` fuzz target's body (ADR 0001 D5 and D8; ADR 0000 P8): a
//! `kai-grants 1` file read back from disk through `kaigrants::parse`, its
//! persistent grants made again as nodes of `captable`'s grant tree as a start
//! does, and the input's bytes read as grant references the way the kernel
//! reads them from a handle's object or a peer's call (`GrantRef::unpack`),
//! each then used on the tree. `fuzz_targets/kai_grants.rs` calls [`grants`]
//! with libFuzzer's input, and `kaigrants/tests/hostile.rs` includes this file
//! and runs it over hostile mutations of the sample here. It must return for
//! any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use captable::grant::{self, GrantRef, NODES, SYSTEM, Tree, USER};
use kaigrants::{Grants, Life, MAX_LINE, MAX_RECORDS, parse};

/// A file with each kind of record: a picker's dir grant, a request's host
/// grant with a signer, and two standing rules, one with no target.
pub const SAMPLE: &str = "kai-grants 1\n\
grant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n\
grant 42 app=mail kind=host object=mail.example.org rights=connect from=request:9 life=session signer=9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f made=2026-11-02T10:15:00Z\n\
rule 57 action=proc_stop target=build-3 from=prompt:212 made=2026-11-02T10:20:00Z\n\
rule 58 action=service_stop target=- from=prompt:213 made=2026-11-02T10:21:00Z\n";

/// The most tree steps one input takes, so a long input stays quick.
const MAX_STEPS: usize = 4 * NODES;

/// The target: the input as a file, then as tree steps.
pub fn grants(bytes: &[u8]) {
    if let Ok(text) = core::str::from_utf8(bytes) {
        let _ = file(text);
    }
    steps(bytes);
}

/// `text` as `kai-grants 1`. What it takes is canonical, writing back byte for
/// byte, and holds no more records or longer lines than the format allows; a
/// refusal names a line of the text. Its persistent grants are made again:
/// how many nodes that made, for a file it takes.
pub fn file(text: &str) -> Option<usize> {
    let grants = match parse(text) {
        Ok(grants) => grants,
        Err(e) => {
            let lines = text.split_inclusive('\n').count().max(1);
            assert!((1..=lines).contains(&e.line), "line {} of {lines}", e.line);
            assert!(!e.why.is_empty());
            return None;
        }
    };
    assert!(grants.grants.len() + grants.rules.len() <= MAX_RECORDS);
    assert!(text.lines().all(|line| line.len() <= MAX_LINE));
    assert_eq!(grants.text().as_deref(), Ok(text), "not canonical");
    let kept = grants.clone().at_boot();
    assert!(kept.grants.iter().all(|g| g.life == Life::Persistent));
    assert_eq!(kept.rules, grants.rules);
    // What a boot keeps writes and reads back as itself.
    let again = kept.text().expect("a kept file writes");
    assert_eq!(parse(&again).as_ref(), Ok(&kept));
    Some(remake(&kept))
}

/// Each persistent grant made again under the user root, as a start does: a
/// node per grant until the tree is full, and full only then.
fn remake(grants: &Grants) -> usize {
    let mut tree = Box::new(Tree::new());
    let user = tree.root(USER);
    for g in &grants.grants {
        let before = tree.len();
        match tree.create(user, 0, grant::Life::Persistent, g.object.as_bytes(), 0) {
            Ok(node) => {
                assert_eq!(tree.len(), before + 1);
                assert!(tree.within(node, user));
                let label = &tree.node(node).expect("a new node is alive").label;
                let n = g.object.len().min(grant::LABEL);
                assert_eq!(&label[..n], &g.object.as_bytes()[..n]);
            }
            Err(e) => {
                assert_eq!(e, grant::Error::Full);
                assert_eq!(before, NODES);
            }
        }
    }
    tree.len() - (usize::from(USER) + 1)
}

/// The input as steps on a tree, nine bytes each: what to do, then a word read
/// as a packed reference or as the index of a node made earlier.
pub fn steps(bytes: &[u8]) {
    let mut tree = Box::new(Tree::new());
    let mut made: Vec<GrantRef> = Vec::new();
    let mut freed = Box::new([0u16; NODES]);
    let (steps, _) = bytes.as_chunks::<9>();
    for step in steps.iter().take(MAX_STEPS) {
        let word = u64::from_le_bytes(step[1..].try_into().expect("eight bytes"));
        let packed = GrantRef::unpack(word);
        // The kernel's reader keeps the low 48 bits: a node and a generation.
        assert_eq!(packed.pack(), word & 0xFFFF_FFFF_FFFF);
        assert_eq!(GrantRef::unpack(packed.pack()), packed);
        let known = match made.is_empty() {
            true => packed,
            false => made[(word % made.len() as u64) as usize],
        };
        let at = match step[0] & 1 {
            0 => packed,
            _ => known,
        };
        look(&tree, at);
        match step[0] >> 1 & 3 {
            0 | 1 => create(&mut tree, &mut made, at, word, &step[1..]),
            2 => revoke(&mut tree, at, &mut freed),
            _ => {
                let other = made.first().copied().unwrap_or(packed);
                let within = tree.within(at, other);
                assert!(!within || (tree.alive(at) && tree.alive(other)));
                assert!(tree.may_give_under(None, at));
                assert_eq!(tree.may_give_under(Some(other), at), within);
            }
        }
        // The roots never go.
        for root in [grant::KERNEL, SYSTEM, USER] {
            assert!(tree.alive(tree.root(root)));
        }
        assert!(tree.len() <= NODES);
    }
}

/// What every reader of a reference sees agrees: alive, its node and its
/// generation.
fn look(tree: &Tree, at: GrantRef) {
    let alive = tree.alive(at);
    assert_eq!(tree.node(at).is_some(), alive);
    // 0 is never a generation a live node has.
    assert!(!alive || at.generation != 0);
    if alive {
        assert_eq!(tree.generation(usize::from(at.node)), at.generation);
        assert!(tree.within(at, tree.root(grant::KERNEL)));
    }
}

fn create(tree: &mut Tree, made: &mut Vec<GrantRef>, parent: GrantRef, word: u64, label: &[u8]) {
    let alive = tree.alive(parent);
    let before = tree.len();
    let life = match word >> 62 {
        0 => grant::Life::Request,
        1 => grant::Life::Session,
        _ => grant::Life::Persistent,
    };
    match tree.create(parent, word, life, label, word >> 1) {
        Ok(node) => {
            assert!(alive, "made under a node that is not alive");
            assert_eq!(tree.len(), before + 1);
            assert!(tree.within(node, parent));
            assert_eq!(tree.node(node).map(|n| n.parent), Some(parent.node));
            made.push(node);
        }
        Err(grant::Error::Revoked) => assert!(!alive),
        Err(grant::Error::Full) => assert_eq!(before, NODES),
        Err(e) => panic!("create answered {e:?}"),
    }
}

fn revoke(tree: &mut Tree, at: GrantRef, freed: &mut [u16; NODES]) {
    let alive = tree.alive(at);
    let before = tree.len();
    match tree.revoke(at, freed) {
        Ok(count) => {
            assert!(alive && at.node > USER);
            assert!(count >= 1);
            assert_eq!(tree.len(), before - count);
            assert!(!tree.alive(at));
            assert!(freed[..count].contains(&at.node));
            for node in &freed[..count] {
                assert!(*node > USER, "a root was freed");
            }
        }
        Err(grant::Error::Revoked) => assert!(!alive),
        Err(grant::Error::Invalid) => assert!(alive && at.node <= USER),
        Err(e) => panic!("revoke answered {e:?}"),
    }
}

/// Tree steps that reach every branch from an empty tree: nodes made under
/// the user root and under each other, references to them read, a subtree
/// revoked, a revoked reference used again and a root revoked.
pub fn sample_steps() -> Vec<u8> {
    let user = GrantRef {
        node: USER,
        generation: 1,
    };
    let mut out = Vec::new();
    let mut step = |what: u8, word: u64| {
        out.push(what);
        out.extend_from_slice(&word.to_le_bytes());
    };
    step(0, user.pack());
    step(1, 0);
    step(1, 1);
    step(6, 0);
    step(5, 1);
    step(5, 1);
    step(4, user.pack());
    step(7, 0);
    out
}
