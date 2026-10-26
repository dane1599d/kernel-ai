//! Design C's three filesystem primitives (ADR 0011 S11-2): write-and-flush,
//! rename-replace and remove, each durable on its own when it returns, with no
//! transaction across them. A crash can fall between any two, and a
//! write-and-flush can be torn, leaving a prefix of the new bytes. Every primitive
//! is logged, so the harness can rebuild the files a crash at any point left.

use crate::{Disk, repo};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// Bytes known to hash to a name, by their address and the name.
type Verified = Rc<std::cell::RefCell<HashMap<(usize, String), Rc<[u8]>>>>;

#[derive(Clone)]
pub enum Prim {
    Write(String, Rc<[u8]>),
    Rename(String, String),
    Remove(String),
}

#[derive(Clone, Default)]
pub struct Prims {
    pub files: BTreeMap<String, Rc<[u8]>>,
    pub log: Vec<Prim>,
    /// Bytes known to hash to a name, by their address and the name: a renamed
    /// file keeps its bytes, so each object is hashed once per sequence. Each
    /// entry holds its bytes, so no other allocation can take their address.
    verified: Verified,
}

impl Prims {
    fn apply(&mut self, prim: &Prim) {
        match prim {
            Prim::Write(path, bytes) => {
                self.files.insert(path.clone(), bytes.clone());
            }
            Prim::Rename(from, to) => {
                if let Some(bytes) = self.files.remove(from) {
                    self.files.insert(to.clone(), bytes);
                }
            }
            Prim::Remove(path) => {
                self.files.remove(path);
            }
        }
    }

    /// The files a crash after the first `done` primitives of `log` left, over
    /// `self` as the base; with `torn`, primitive `done` is a write cut in half.
    pub fn crashed(&self, log: &[Prim], done: usize, torn: bool) -> Prims {
        let mut state = Prims {
            files: self.files.clone(),
            log: Vec::new(),
            verified: self.verified.clone(),
        };
        for prim in &log[..done] {
            state.apply(prim);
        }
        if torn && let Some(Prim::Write(path, bytes)) = log.get(done) {
            let half: Rc<[u8]> = bytes[..bytes.len() / 2].into();
            state.files.insert(path.clone(), half);
        }
        state
    }

    fn log(&mut self, prim: Prim) {
        self.apply(&prim);
        self.log.push(prim);
    }
}

impl Disk for Prims {
    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.files.get(path).map(|bytes| bytes.to_vec()))
    }

    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let prefix = format!("{dir}/");
        Ok(self
            .files
            .keys()
            .filter_map(|path| path.strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(String::from)
            .collect())
    }

    fn write(&mut self, path: &str, bytes: &[u8]) {
        self.log(Prim::Write(path.into(), bytes.into()));
    }

    fn rename(&mut self, from: &str, to: &str) {
        self.log(Prim::Rename(from.into(), to.into()));
    }

    fn remove(&mut self, path: &str) {
        self.log(Prim::Remove(path.into()));
    }

    fn verified(&mut self, path: &str, sha: &str) -> Result<bool, String> {
        let Some(bytes) = self.files.get(path) else {
            return Ok(false);
        };
        let key = (bytes.as_ptr() as usize, sha.to_string());
        if self
            .verified
            .borrow()
            .get(&key)
            .is_some_and(|held| held.len() == bytes.len())
        {
            return Ok(true);
        }
        let good = repo::sha(bytes) == sha;
        if good {
            self.verified.borrow_mut().insert(key, bytes.clone());
        }
        Ok(good)
    }
}
