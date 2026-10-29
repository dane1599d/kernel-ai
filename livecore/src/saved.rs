//! An app's state kept on disk across reboots (P4, app storage that survives a
//! reboot). The kernel keeps a component's checkpoint and storage in RAM only,
//! so comphost also writes each committed checkpoint into the app's own folder,
//! `/system/apps/<app>/`, and starts from it when the kernel has none.
//!
//! A file write goes out in pieces and is not atomic, so there are two slots,
//! `state.0` and `state.1`, written in turn. Each is a [`Record`]: a generation,
//! the schema, the bytes and a checksum over them. [`newest`] picks the valid
//! slot with the higher generation, so a crash in the middle of a write loses
//! at most that one checkpoint, never the one before it.

use alloc::vec::Vec;

/// The slots' file names, by `generation % 2`.
pub const SLOTS: [&str; 2] = ["state.0", "state.1"];
const MAGIC: &[u8; 8] = b"kaistat1";
/// Magic, generation, schema, length and checksum.
const HEAD: usize = 8 + 8 + 4 + 4 + 8;

/// One saved checkpoint: what the kernel would hold, joined with the storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub generation: u64,
    pub schema: u32,
    pub checkpoint: Vec<u8>,
}

/// FNV-1a over `bytes`, salted with the head's fields so a stale head cannot
/// pass with other bytes.
fn checksum(generation: u64, schema: u32, bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    generation.to_le_bytes().into_iter().for_each(&mut eat);
    schema.to_le_bytes().into_iter().for_each(&mut eat);
    bytes.iter().copied().for_each(eat);
    h
}

impl Record {
    /// The slot this record goes to.
    pub fn slot(&self) -> &'static str {
        SLOTS[(self.generation % 2) as usize]
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEAD + self.checkpoint.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&self.schema.to_le_bytes());
        out.extend_from_slice(&(self.checkpoint.len() as u32).to_le_bytes());
        let sum = checksum(self.generation, self.schema, &self.checkpoint);
        out.extend_from_slice(&sum.to_le_bytes());
        out.extend_from_slice(&self.checkpoint);
        out
    }

    /// A slot's bytes, if they hold a whole, unchanged record no larger than
    /// `max` bytes of checkpoint.
    pub fn decode(bytes: &[u8], max: usize) -> Option<Self> {
        let head = bytes.get(..HEAD)?;
        if &head[..8] != MAGIC {
            return None;
        }
        let word = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap_or([0; 8]));
        let generation = word(8);
        let schema = u32::from_le_bytes(head[16..20].try_into().ok()?);
        let len = u32::from_le_bytes(head[20..24].try_into().ok()?) as usize;
        let sum = word(24);
        if len > max {
            return None;
        }
        let checkpoint = bytes.get(HEAD..HEAD + len)?;
        (checksum(generation, schema, checkpoint) == sum).then(|| Self {
            generation,
            schema,
            checkpoint: checkpoint.to_vec(),
        })
    }
}

/// What the host starts from: the kernel's ledger, or, when the kernel has
/// nothing (after a reboot), the newest saved record.
pub fn merge(restored: &mut crate::host::Restored, saved: Option<Record>) {
    if restored.phase != kabi::live::phase::EMPTY {
        return;
    }
    if let Some(record) = saved {
        restored.schema = u64::from(record.schema);
        restored.checkpoint = record.checkpoint;
    }
}

/// The newest valid record of the two slots' bytes.
pub fn newest(slots: [Option<&[u8]>; 2], max: usize) -> Option<Record> {
    slots
        .into_iter()
        .flatten()
        .filter_map(|bytes| Record::decode(bytes, max))
        .max_by_key(|r| r.generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(generation: u64, text: &[u8]) -> Record {
        Record {
            generation,
            schema: 3,
            checkpoint: text.to_vec(),
        }
    }

    #[test]
    fn a_record_round_trips_and_goes_to_its_slot() {
        let r = record(7, b"state and storage");
        assert_eq!(Record::decode(&r.encode(), 1024), Some(r.clone()));
        assert_eq!((r.slot(), record(8, b"").slot()), ("state.1", "state.0"));
    }

    #[test]
    fn a_torn_or_changed_slot_is_not_read_and_the_other_wins() {
        let old = record(4, b"the last good state").encode();
        let new = record(5, b"the state being written").encode();
        // Cut short mid-write, or a byte changed: refused.
        let torn = &new[..new.len() - 3];
        let mut flipped = new.clone();
        *flipped.last_mut().unwrap() ^= 1;
        for bad in [torn, flipped.as_slice(), b"", b"kaistat1"] {
            assert_eq!(
                newest([Some(&old), Some(bad)], 1024).map(|r| r.generation),
                Some(4)
            );
        }
        assert_eq!(
            newest([Some(&old), Some(&new)], 1024).unwrap().generation,
            5
        );
        assert_eq!(newest([None, None], 1024), None);
        // Over the cap: refused, not read.
        assert_eq!(Record::decode(&new, 4), None);
    }
}
