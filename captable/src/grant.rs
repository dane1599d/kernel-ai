//! The grant tree (ADR 0001 D5): every handle given under a grant records its
//! node, and revoking a node revokes it and everything below it at once.
//!
//! [`NODES`] nodes in one tree with three roots: `kernel`, `system` (the
//! supervisor) and `user` (the trusted path). A node has a parent, an owner, a
//! 24-byte label, a life (`request`, `session` or `persistent`) and an expiry.
//! Each slot keeps a generation that a [`GrantRef`] carries: revoking bumps the
//! generation of the node and of every descendant and frees them, so checking a
//! handle is one comparison of generations. A revoke scans the tree once.

/// Nodes in the tree.
pub const NODES: usize = 1024;
/// The three roots.
pub const KERNEL: u16 = 0;
pub const SYSTEM: u16 = 1;
pub const USER: u16 = 2;
/// Bytes of a node's label.
pub const LABEL: usize = 24;

/// A node at the generation it had when a handle was given under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GrantRef {
    pub node: u16,
    pub generation: u32,
}

impl GrantRef {
    /// Packed into one u64, as a `Grant` handle's object.
    pub const fn pack(self) -> u64 {
        ((self.generation as u64) << 16) | self.node as u64
    }

    pub const fn unpack(object: u64) -> GrantRef {
        GrantRef {
            node: (object & 0xFFFF) as u16,
            generation: (object >> 16) as u32,
        }
    }
}

/// Whether a grant node is still alive: the kernel's view of the tree.
pub trait Live {
    fn alive(&self, grant: GrantRef) -> bool;
}

/// Every grant alive, for handles that never have one (and tests).
pub struct Always;

impl Live for Always {
    fn alive(&self, _: GrantRef) -> bool {
        true
    }
}

/// How long a grant lasts (ADR 0001 D5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    /// One typed request: revoked when its result line is written.
    Request,
    /// Until the session ends.
    Session,
    /// Kept as a record in `kai-grants 1` and made again at each start.
    Persistent,
}

/// One node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub parent: u16,
    /// The pid that made it (0 for the kernel).
    pub owner: u64,
    pub label: [u8; LABEL],
    pub life: Life,
    /// When it lapses, in the kernel's nanoseconds; 0 for never.
    pub expiry_ns: u64,
}

/// Why a tree operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The node, or the parent asked for, is not alive.
    Revoked,
    /// All [`NODES`] are in use.
    Full,
    /// A root cannot be revoked.
    Invalid,
}

pub struct Tree {
    nodes: [Option<Node>; NODES],
    generations: [u32; NODES],
}

impl Default for Tree {
    fn default() -> Self {
        Tree::new()
    }
}

impl Tree {
    /// A tree with its three roots.
    pub const fn new() -> Tree {
        /// A root named `label`.
        const fn root(label: &[u8]) -> Option<Node> {
            let mut bytes = [0u8; LABEL];
            let mut i = 0;
            while i < label.len() {
                bytes[i] = label[i];
                i += 1;
            }
            Some(Node {
                parent: KERNEL,
                owner: 0,
                label: bytes,
                life: Life::Persistent,
                expiry_ns: 0,
            })
        }
        let mut nodes = [None; NODES];
        nodes[KERNEL as usize] = root(b"kernel");
        nodes[SYSTEM as usize] = root(b"system");
        nodes[USER as usize] = root(b"user");
        Tree {
            nodes,
            generations: [1; NODES],
        }
    }

    /// The root `node` as a reference.
    pub fn root(&self, node: u16) -> GrantRef {
        GrantRef {
            node,
            generation: self.generations[usize::from(node)],
        }
    }

    pub fn alive(&self, grant: GrantRef) -> bool {
        let index = usize::from(grant.node);
        index < NODES && self.nodes[index].is_some() && self.generations[index] == grant.generation
    }

    /// A slot's generation, for a lock-free copy of the tree's liveness.
    pub fn generation(&self, node: usize) -> u32 {
        self.generations[node]
    }

    pub fn node(&self, grant: GrantRef) -> Option<&Node> {
        self.alive(grant)
            .then(|| self.nodes[usize::from(grant.node)].as_ref())
            .flatten()
    }

    /// Nodes in use, the roots included.
    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A new node under `parent`, which must be alive.
    pub fn create(
        &mut self,
        parent: GrantRef,
        owner: u64,
        life: Life,
        label: &[u8],
        expiry_ns: u64,
    ) -> Result<GrantRef, Error> {
        if !self.alive(parent) {
            return Err(Error::Revoked);
        }
        let index = self
            .nodes
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Full)?;
        let mut bytes = [0u8; LABEL];
        let n = label.len().min(LABEL);
        bytes[..n].copy_from_slice(&label[..n]);
        self.nodes[index] = Some(Node {
            parent: parent.node,
            owner,
            label: bytes,
            life,
            expiry_ns,
        });
        Ok(GrantRef {
            node: index as u16,
            generation: self.generations[index],
        })
    }

    /// A live node `owner` made, the first by index.
    pub fn owned_by(&self, owner: u64) -> Option<GrantRef> {
        let index = self
            .nodes
            .iter()
            .enumerate()
            .position(|(i, n)| i > usize::from(USER) && n.is_some_and(|n| n.owner == owner))?;
        Some(GrantRef {
            node: index as u16,
            generation: self.generations[index],
        })
    }

    /// Whether `node` is `ancestor` or below it.
    pub fn within(&self, node: GrantRef, ancestor: GrantRef) -> bool {
        if !self.alive(node) || !self.alive(ancestor) {
            return false;
        }
        let mut at = node.node;
        for _ in 0..NODES {
            if at == ancestor.node {
                return true;
            }
            match self.nodes[usize::from(at)] {
                Some(n) if at != KERNEL => at = n.parent,
                _ => return false,
            }
        }
        false
    }

    /// Revokes `grant` and every node below it: each is freed and its slot's
    /// generation bumped, so no handle given under any of them works again.
    /// Writes the freed nodes to `freed[..count]`. Roots stay. Not generic, so
    /// it runs as compiled here (optimised) whoever calls it.
    pub fn revoke(&mut self, grant: GrantRef, freed: &mut [u16; NODES]) -> Result<usize, Error> {
        if !self.alive(grant) {
            return Err(Error::Revoked);
        }
        if grant.node <= USER {
            return Err(Error::Invalid);
        }
        // Mark first, then free, so a freed parent does not hide a descendant.
        let mut doomed = [false; NODES];
        for index in 0..NODES {
            if self.nodes[index].is_none() {
                continue;
            }
            let mut at = index as u16;
            for _ in 0..NODES {
                if at == grant.node {
                    doomed[index] = true;
                    break;
                }
                match self.nodes[usize::from(at)] {
                    Some(n) if at > USER => at = n.parent,
                    _ => break,
                }
            }
        }
        let mut count = 0;
        for (index, doomed) in doomed.iter().enumerate() {
            if *doomed {
                self.nodes[index] = None;
                // 0 is never a generation a reference carries.
                self.generations[index] = self.generations[index].wrapping_add(1).max(1);
                freed[count] = index as u16;
                count += 1;
            }
        }
        Ok(count)
    }
}

impl Live for Tree {
    fn alive(&self, grant: GrantRef) -> bool {
        Tree::alive(self, grant)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::boxed::Box;

    #[test]
    fn revoking_a_node_revokes_everything_below_it_and_nothing_else() {
        let mut tree = Box::new(Tree::new());
        let user = tree.root(USER);
        let request = tree
            .create(user, 0, Life::Request, b"request 1", 0)
            .unwrap();
        let child = tree.create(request, 5, Life::Request, b"call", 0).unwrap();
        let grandchild = tree.create(child, 5, Life::Request, b"script", 0).unwrap();
        let other = tree
            .create(user, 0, Life::Request, b"request 2", 0)
            .unwrap();
        assert!(tree.within(grandchild, request) && !tree.within(other, request));
        let mut freed = [0u16; NODES];
        assert_eq!(tree.revoke(request, &mut freed), Ok(3));
        assert_eq!(freed[..3], [request.node, child.node, grandchild.node]);
        for gone in [request, child, grandchild] {
            assert!(!tree.alive(gone));
        }
        assert!(tree.alive(other) && tree.alive(user));
        assert_eq!(tree.revoke(request, &mut [0; NODES]), Err(Error::Revoked));
        assert_eq!(
            tree.create(request, 0, Life::Request, b"x", 0),
            Err(Error::Revoked)
        );
        // A reused slot gets a new generation, so the old reference stays dead.
        let again = tree
            .create(user, 0, Life::Request, b"request 3", 0)
            .unwrap();
        assert!(!tree.alive(request) || again != request);
    }

    #[test]
    fn roots_cannot_be_revoked_and_the_tree_fills_up() {
        let mut tree = Box::new(Tree::new());
        for root in [KERNEL, SYSTEM, USER] {
            let r = tree.root(root);
            assert_eq!(tree.revoke(r, &mut [0; NODES]), Err(Error::Invalid));
        }
        let user = tree.root(USER);
        for _ in 3..NODES {
            tree.create(user, 0, Life::Session, b"s", 0).unwrap();
        }
        assert_eq!(
            tree.create(user, 0, Life::Session, b"s", 0),
            Err(Error::Full)
        );
        assert_eq!(tree.len(), NODES);
    }

    #[test]
    fn a_reference_packs_into_an_object() {
        let grant = GrantRef {
            node: 1023,
            generation: 0xABCD_1234,
        };
        assert_eq!(GrantRef::unpack(grant.pack()), grant);
    }
}
