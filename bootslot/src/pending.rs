//! `/system/updates/pending`, the update in flight (ADR 0008 §10): written by
//! `updated` after the `update` snapshot and before the restart into the new
//! slot, as `snapshot <id> slot <A|B> release <seq>`.

use crate::Slot;
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub snapshot: u64,
    pub slot: Slot,
    pub release_seq: u32,
}

impl fmt::Display for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let slot = if self.slot == Slot::A { "A" } else { "B" };
        writeln!(
            f,
            "snapshot {} slot {slot} release {}",
            self.snapshot, self.release_seq
        )
    }
}

impl Pending {
    pub fn parse(text: &str) -> Option<Pending> {
        let mut words = text.split_whitespace();
        let mut field = |key| (words.next() == Some(key)).then(|| words.next()).flatten();
        let snapshot = field("snapshot")?.parse().ok()?;
        let slot = match field("slot")? {
            "A" => Slot::A,
            "B" => Slot::B,
            _ => return None,
        };
        let release_seq = field("release")?.parse().ok()?;
        if words.next().is_some() {
            return None;
        }
        Some(Pending {
            snapshot,
            slot,
            release_seq,
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::format;

    #[test]
    fn pending_reads_back_what_it_wrote() {
        for slot in [Slot::A, Slot::B] {
            let pending = Pending {
                snapshot: 7,
                slot,
                release_seq: u32::MAX,
            };
            let text = format!("{pending}");
            assert!(text.ends_with('\n'));
            assert_eq!(Pending::parse(&text), Some(pending));
        }
    }

    #[test]
    fn anything_else_in_pending_is_refused() {
        for text in [
            "",
            "snapshot 7 slot R release 2",
            "snapshot 7 slot A",
            "snapshot x slot A release 2",
            "release 2 slot A snapshot 7",
            "snapshot 7 slot A release 2 more",
            "snapshot 7 slot A release 4294967296",
        ] {
            assert_eq!(Pending::parse(text), None, "{text:?}");
        }
    }
}
