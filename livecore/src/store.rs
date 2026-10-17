//! The kernel's side of the COMP_* calls for one component, as plain data: its
//! versions, how often each was fetched, and its [`Ledger`]. The kernel keeps one
//! per component; tests and xtask use the same code as a fake kernel.

use crate::ledger::{Event, Ledger, Phase};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{MAX_ARTIFACT, MAX_LABEL, MAX_VERSIONS, Restore, phase, valid_label};

/// A version's bytes: a boot module, or (for the edit loop) bytes on the heap.
#[derive(Clone, Debug)]
pub enum Bytes {
    Static(&'static [u8]),
    Heap(Vec<u8>),
}

impl Bytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Heap(bytes) => bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    versions: Vec<(String, Bytes, u64)>,
    pub ledger: Ledger,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            versions: Vec::new(),
            ledger: Ledger::new(),
        }
    }

    pub fn add_version(&mut self, label: &str, bytes: Bytes) -> Result<(), &'static str> {
        if !valid_label(label) {
            return Err("bad version label");
        }
        if bytes.as_slice().len() > MAX_ARTIFACT {
            return Err("artifact over 1 MiB");
        }
        if self.versions.len() == MAX_VERSIONS {
            return Err("too many versions");
        }
        if self.versions.iter().any(|(l, ..)| l == label) {
            return Err("version listed twice");
        }
        self.versions.push((label.to_string(), bytes, 0));
        Ok(())
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.versions.iter().map(|(label, ..)| label.as_str())
    }

    /// Version `label` for process `pid`, when it fits in `cap` bytes; its length
    /// either way. Only a fetch that fits counts, and lets `pid` report on it.
    /// Version `label`'s bytes.
    pub fn bytes(&self, label: &str) -> Result<&Bytes, Error> {
        if !valid_label(label) {
            return Err(Error::Invalid);
        }
        self.versions
            .iter()
            .find(|(l, ..)| l == label)
            .map(|(_, bytes, _)| bytes)
            .ok_or(Error::NotFound)
    }

    /// Counts a fetch of `label` by `pid`, which may then report on it.
    pub fn record_fetch(&mut self, pid: u64, label: &str) {
        if let Some((.., count)) = self.versions.iter_mut().find(|(l, ..)| l == label) {
            *count += 1;
            self.ledger.fetched(pid, label);
        }
    }

    pub fn fetch(&mut self, pid: u64, label: &str, cap: usize) -> Result<(&[u8], bool), Error> {
        let len = self.bytes(label)?.as_slice().len();
        let fits = cap >= len;
        if fits {
            self.record_fetch(pid, label);
        }
        Ok((self.bytes(label)?.as_slice(), fits))
    }

    pub fn fetches(&self, label: &str) -> u64 {
        self.versions
            .iter()
            .find(|(l, ..)| l == label)
            .map_or(0, |(.., count)| *count)
    }

    pub fn event(&mut self, pid: u64, event: &Event) -> Result<Option<String>, Error> {
        self.ledger.apply(pid, event)
    }

    /// The ledger as [`Restore`] reports it.
    pub fn restore(&self) -> Restore {
        let mut restore = Restore::empty();
        let ledger = &self.ledger;
        restore.phase = match ledger.phase {
            Phase::Empty => phase::EMPTY,
            Phase::Steady { .. } => phase::STEADY,
            Phase::Swapping { .. } => phase::SWAPPING,
        };
        restore.schema = ledger.schema;
        restore.generation = ledger.generation;
        restore.checkpoint_len = ledger.checkpoint.len() as u64;
        restore.flags = if ledger.disabled {
            Restore::DISABLED
        } else {
            0
        };
        let copy = |into: &mut [u8; MAX_LABEL], label: Option<&str>| {
            let bytes = label.unwrap_or("").as_bytes();
            into[..bytes.len()].copy_from_slice(bytes);
            bytes.len() as u64
        };
        restore.active_len = copy(&mut restore.active, ledger.active());
        restore.candidate_len = copy(&mut restore.candidate, ledger.candidate());
        let name = self.name.as_bytes();
        let len = name.len().min(kabi::MAX_NAME);
        restore.name[..len].copy_from_slice(&name[..len]);
        restore.name_len = len as u64;
        restore
    }

    pub fn checkpoint(&self) -> &[u8] {
        &self.ledger.checkpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kabi::live::kind;

    #[test]
    fn a_fetch_counts_only_when_it_fits_and_lets_the_host_report() {
        let mut notes = Component::new("notes");
        notes
            .add_version("1", Bytes::Static(b"module one"))
            .unwrap();
        assert_eq!(
            notes.add_version("1", Bytes::Static(b"")),
            Err("version listed twice")
        );
        assert_eq!(
            notes.add_version("Bad", Bytes::Static(b"")),
            Err("bad version label")
        );
        assert_eq!(notes.fetch(5, "1", 3), Ok((&b"module one"[..], false)));
        assert_eq!(notes.fetches("1"), 0);
        assert_eq!(notes.fetch(5, "2", 100), Err(Error::NotFound));
        assert_eq!(notes.fetch(5, "../x", 100), Err(Error::Invalid));
        assert!(notes.fetch(5, "1", 100).unwrap().1);
        assert_eq!(notes.fetches("1"), 1);
        let restored = Event {
            kind: kind::RESTORED,
            label: "1".into(),
            schema: 1,
            blob: alloc::vec![0x82, 1, 0x80],
            ..Event::default()
        };
        assert!(notes.event(5, &restored).is_ok());
        let restore = notes.restore();
        assert_eq!(
            (restore.phase, restore.active(), restore.checkpoint_len),
            (phase::STEADY, "1", 3)
        );
    }
}
