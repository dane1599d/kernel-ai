//! What `cargo xtask doctor` says about the model store: for each catalog model,
//! whether the store holds the pinned file, by size and then SHA-256. Only reads:
//! a partial download is left for `setup` to resume, and a wrong file is reported,
//! never deleted.

use super::catalog::{Catalog, Entry};
use super::{fetch, store};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    Missing,
    /// A `.part` download of this many bytes, which `setup` resumes.
    Partial(u64),
    /// A file of another size than the catalog pins.
    WrongSize(u64),
    /// The pinned size, but another SHA-256.
    Corrupt(String),
    /// The file could not be read to the end.
    Unreadable(String),
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stocked {
    pub id: String,
    pub name: String,
    /// The file name in the store.
    pub file: String,
    pub size: u64,
    pub held: Held,
    /// The least RAM, in GB, the catalog recommends it for.
    pub min_ram_gb: u64,
    /// Whether the engine runs all its weight types (`Entry::not_runnable`).
    pub runnable: bool,
    /// Guest memory `run --local` gives a VM with it, in MiB.
    pub vm_mib: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    pub dir: PathBuf,
    pub default: String,
    pub models: Vec<Stocked>,
}

impl Inventory {
    /// The model `setup` picks for this much RAM: the largest runnable one the
    /// catalog recommends for it, else the default when the RAM is unknown.
    pub fn recommend(&self, ram_bytes: Option<u64>) -> Option<&Stocked> {
        let Some(bytes) = ram_bytes else {
            return self.models.iter().find(|m| m.id == self.default);
        };
        // Rounded as `setup` rounds it: 31.9 GiB visible is 32 GB.
        let ram_gb = (bytes + (1 << 29)) >> 30;
        self.models
            .iter()
            .filter(|m| m.runnable && m.min_ram_gb <= ram_gb)
            .max_by_key(|m| (m.min_ram_gb, m.size))
    }
}

/// The store `setup` would use (`--store`, else `KAI_MODELS`, else the default)
/// and what it holds.
pub fn inventory(store_flag: Option<&str>) -> Result<Inventory, String> {
    let catalog = Catalog::load()?;
    let dir = store::resolve(store_flag)?;
    let models = catalog
        .models
        .iter()
        .map(|entry| Stocked {
            id: entry.id.clone(),
            name: entry.name.clone(),
            file: entry.file.clone(),
            size: entry.size,
            held: held(&dir, entry),
            min_ram_gb: entry.min_ram_gb,
            runnable: entry.not_runnable().is_none(),
            vm_mib: crate::disk::memory_mib_for(entry.size),
        })
        .collect();
    Ok(Inventory {
        dir,
        default: catalog.default,
        models,
    })
}

fn held(dir: &Path, entry: &Entry) -> Held {
    let path = dir.join(&entry.file);
    match fs::metadata(&path) {
        Ok(meta) if meta.len() != entry.size => Held::WrongSize(meta.len()),
        Ok(_) => match fetch::sha256_file(&path) {
            Ok(sha) if sha == entry.sha256 => Held::Verified,
            Ok(sha) => Held::Corrupt(sha),
            Err(e) => Held::Unreadable(e),
        },
        Err(_) => match fs::metadata(dir.join(format!("{}.part", entry.file))) {
            Ok(meta) => Held::Partial(meta.len()),
            Err(_) => Held::Missing,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_of_a_stored_file_is_told_apart() {
        let dir = std::env::temp_dir().join(format!("kai-inventory-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut entry = Catalog::load().unwrap().models[0].clone();
        entry.file = "m.gguf".into();
        entry.size = 3;
        // SHA-256 of "abc".
        entry.sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into();
        assert_eq!(held(&dir, &entry), Held::Missing);
        fs::write(dir.join("m.gguf.part"), b"a").unwrap();
        assert_eq!(held(&dir, &entry), Held::Partial(1));
        fs::write(dir.join("m.gguf"), b"abcd").unwrap();
        assert_eq!(held(&dir, &entry), Held::WrongSize(4));
        fs::write(dir.join("m.gguf"), b"abd").unwrap();
        assert!(matches!(held(&dir, &entry), Held::Corrupt(_)));
        fs::write(dir.join("m.gguf"), b"abc").unwrap();
        assert_eq!(held(&dir, &entry), Held::Verified);
        fs::remove_dir_all(&dir).unwrap();
    }
}
