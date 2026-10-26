//! The store's files on S04-1's spike kaifs (`spikes/kaifs`): one transaction
//! from `begin` to `commit`, two flushes and both superblock rings per commit.
//! The spike engine has u64 keys and no namespace yet, so a file is the key
//! FNV-1a(path) holding its path and bytes (inline under 1 KiB, else a data record
//! of up to 128 KiB), and each directory is a key holding its names. Each commit
//! also writes a sequence number, so a crash state names the commit it holds.

use crate::Disk;
use crate::store::{self, Store, Summary};
use spike_kaifs::{BlockDev, Fs, MAX_RECORD, MAX_VALUE, Value};
use std::collections::BTreeMap;

/// The key holding the number of the newest commit.
pub const SEQ_KEY: u64 = 1;

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    }) | 2
}

fn file_key(path: &str) -> u64 {
    fnv(path)
}

fn dir_key(dir: &str) -> u64 {
    fnv(&format!("\u{1}dir {dir}"))
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

pub struct Kaifs<D: BlockDev> {
    pub fs: Fs<D>,
    /// The next commit's sequence number.
    pub seq: u64,
    /// With a store, each commit checks the store's invariants on the live disk
    /// and keeps what it held, by sequence number, for the crash states to match.
    pub check: Option<Store>,
    pub summaries: BTreeMap<u64, Result<Summary, String>>,
}

impl<D: BlockDev> Kaifs<D> {
    pub fn new(fs: Fs<D>) -> Self {
        Kaifs {
            fs,
            seq: 1,
            check: None,
            summaries: BTreeMap::new(),
        }
    }

    /// The sequence number of the commit this filesystem holds.
    pub fn committed_seq(&mut self) -> Result<u64, String> {
        match self.fs.get(SEQ_KEY).map_err(|e| format!("{e:?}"))? {
            Some(Value::Inline(bytes)) if bytes.len() == 8 => {
                Ok(u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])))
            }
            _ => Ok(0),
        }
    }

    fn raw(&mut self, key: u64) -> Result<Option<Vec<u8>>, String> {
        Ok(
            match self.fs.get(key).map_err(|e| format!("read: {e:?}"))? {
                Some(Value::Inline(bytes) | Value::Record(bytes)) => Some(bytes),
                None => None,
            },
        )
    }

    fn set(&mut self, key: u64, value: &[u8]) {
        let result = if value.len() < MAX_VALUE {
            self.fs.put(key, value)
        } else {
            assert!(value.len() <= MAX_RECORD, "a file over 128 KiB");
            self.fs.put_record(key, value)
        };
        result.expect("kaifs: a write failed");
    }

    fn names(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let raw = self.raw(dir_key(dir))?.unwrap_or_default();
        let text = String::from_utf8(raw).map_err(|_| format!("directory {dir} is not text"))?;
        Ok(text.lines().map(String::from).collect())
    }

    fn set_names(&mut self, dir: &str, names: &[String]) {
        let text: String = names.iter().map(|n| format!("{n}\n")).collect();
        self.set(dir_key(dir), text.as_bytes());
    }

    fn link(&mut self, path: &str) {
        let (dir, name) = split(path);
        let mut names = self.names(dir).expect("kaifs: a directory read failed");
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
            self.set_names(dir, &names);
        }
    }

    fn unlink(&mut self, path: &str) {
        let (dir, name) = split(path);
        let mut names = self.names(dir).expect("kaifs: a directory read failed");
        names.retain(|n| n != name);
        self.set_names(dir, &names);
    }
}

impl<D: BlockDev> Disk for Kaifs<D> {
    fn commit(&mut self) {
        let seq = self.seq;
        self.set(SEQ_KEY, &seq.to_le_bytes());
        self.fs.commit().expect("kaifs: a commit failed");
        self.seq += 1;
        if let Some(store) = self.check {
            let summary = store::check(self, &store);
            self.summaries.insert(seq, summary);
        }
    }

    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(raw) = self.raw(file_key(path))? else {
            return Ok(None);
        };
        let len = raw.first().map_or(0, |&b| b as usize);
        if raw.get(1..1 + len) != Some(path.as_bytes()) {
            return Err(format!("the key of {path} holds another file"));
        }
        Ok(Some(raw[1 + len..].to_vec()))
    }

    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        self.names(dir)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) {
        assert!(path.len() < 256);
        let mut value = vec![path.len() as u8];
        value.extend_from_slice(path.as_bytes());
        value.extend_from_slice(bytes);
        self.set(file_key(path), &value);
        self.link(path);
    }

    fn rename(&mut self, from: &str, to: &str) {
        let bytes = self
            .read(from)
            .ok()
            .flatten()
            .expect("kaifs: rename of a missing file");
        self.write(to, &bytes);
        self.remove(from);
    }

    fn remove(&mut self, path: &str) {
        self.fs
            .delete(file_key(path))
            .expect("kaifs: a delete failed");
        self.unlink(path);
    }
}
