//! The store's [`Volume`] in memory: where later generations live until reboot
//! when there is no `kai-data` disk (ADR 0011 §5), and the host tests' volume
//! where a real filesystem would only slow them down. A transaction keeps a copy
//! of the files from [`Volume::begin`], which [`MemVolume::abort`] puts back.

use crate::store::Volume;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, Default)]
pub struct MemVolume {
    files: BTreeMap<String, Vec<u8>>,
    /// The files as `begin` found them, while a transaction is open.
    before: Option<BTreeMap<String, Vec<u8>>>,
}

impl MemVolume {
    /// Ends an open transaction with nothing of it kept, as a crash before its
    /// commit does on disk.
    pub fn abort(&mut self) {
        if let Some(before) = self.before.take() {
            self.files = before;
        }
    }

    /// Every file, by path.
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

impl Volume for MemVolume {
    fn begin(&mut self, _action: u64) -> Result<(), String> {
        if self.before.is_some() {
            return Err("a transaction is already open".into());
        }
        self.before = Some(self.files.clone());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), String> {
        self.before = None;
        Ok(())
    }

    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.files.get(path).cloned())
    }

    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let prefix = alloc::format!("{dir}/");
        let mut names: Vec<String> = self
            .files
            .keys()
            .filter_map(|path| path.strip_prefix(&prefix))
            .map(|rest| rest.split('/').next().unwrap_or(rest).to_string())
            .collect();
        names.dedup();
        Ok(names)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.files.insert(path.into(), bytes.to_vec());
        Ok(())
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.files
            .entry(path.into())
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let bytes = self
            .files
            .remove(from)
            .ok_or_else(|| alloc::format!("no {from} to rename"))?;
        self.files.insert(to.into(), bytes);
        Ok(())
    }

    fn remove(&mut self, path: &str) -> Result<(), String> {
        self.files.remove(path);
        Ok(())
    }
}
