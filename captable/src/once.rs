//! `ONCE` handles' digests (ADR 0001 D6): the trusted path mints a `ONCE`
//! handle under a request node once the user said yes, and keeps here the full
//! SHA-256 over what the question showed (the log entry id, the action and its
//! arguments). The handle's badge names the digest, not the digest itself: a
//! 64-bit badge alone could be collided in about 2^32 tries. The action's owner
//! asks whether the digest of the call it got matches, and refuses it if not.
//! A digest goes with its request node, and with a first match.

use crate::grant::{GrantRef, Live};

/// Digests held at once.
pub const DIGESTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    badge: u64,
    grant: GrantRef,
    digest: [u8; 32],
}

/// What a check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The digest matches; it is now used up.
    Match,
    /// No digest under that badge (never held, or used up).
    Unknown,
    /// Another digest: what runs is not what was shown.
    Mismatch,
    /// Its request node was revoked.
    Revoked,
}

pub struct Book {
    held: [Option<Held>; DIGESTS],
    next: u64,
}

impl Default for Book {
    fn default() -> Self {
        Book::new()
    }
}

impl Book {
    pub const fn new() -> Book {
        Book {
            held: [None; DIGESTS],
            next: 1,
        }
    }

    /// Holds `digest` for a `ONCE` handle under `grant`; the badge that names
    /// it, never reused. `None` when all [`DIGESTS`] are held by live grants.
    pub fn hold(&mut self, grant: GrantRef, digest: [u8; 32], live: &dyn Live) -> Option<u64> {
        let free = self
            .held
            .iter()
            .position(|h| h.is_none_or(|h| !live.alive(h.grant)))?;
        let badge = self.next;
        self.next += 1;
        self.held[free] = Some(Held {
            badge,
            grant,
            digest,
        });
        Some(badge)
    }

    /// Whether the call the owner got, digested as `digest`, is the one shown
    /// for `badge`. A match uses the digest up, so it matches once.
    pub fn check(&mut self, badge: u64, digest: &[u8; 32], live: &dyn Live) -> Verdict {
        let Some(slot) = self
            .held
            .iter()
            .position(|h| h.is_some_and(|h| h.badge == badge))
        else {
            return Verdict::Unknown;
        };
        let held = self.held[slot].unwrap_or_else(|| unreachable!());
        if !live.alive(held.grant) {
            self.held[slot] = None;
            return Verdict::Revoked;
        }
        // Compared in full, every byte, so the time says nothing of where they part.
        let differ = held
            .digest
            .iter()
            .zip(digest)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if differ != 0 {
            return Verdict::Mismatch;
        }
        self.held[slot] = None;
        Verdict::Match
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::grant::{Life, Tree, USER};
    use std::boxed::Box;

    #[test]
    fn a_digest_matches_once_and_only_while_its_request_lives() {
        let mut tree = Box::new(Tree::new());
        let mut book = Book::new();
        let request = tree
            .create(tree.root(USER), 0, Life::Request, b"request 7", 0)
            .unwrap();
        let shown = [7u8; 32];
        let badge = book.hold(request, shown, &*tree).unwrap();
        let mut other = shown;
        other[31] ^= 1;
        assert_eq!(book.check(badge, &other, &*tree), Verdict::Mismatch);
        assert_eq!(book.check(badge + 1, &shown, &*tree), Verdict::Unknown);
        assert_eq!(book.check(badge, &shown, &*tree), Verdict::Match);
        assert_eq!(book.check(badge, &shown, &*tree), Verdict::Unknown);
        let badge = book.hold(request, shown, &*tree).unwrap();
        tree.revoke(request, &mut [false; crate::grant::NODES])
            .unwrap();
        assert_eq!(book.check(badge, &shown, &*tree), Verdict::Revoked);
    }
}
