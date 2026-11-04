//! The `kai-chunks 1` lists the `models` role signs beside each catalog model
//! (P5 task 6, ADR 0008 §9): `xtask/models/chunks/<sha256>.chunks`, committed
//! like the goldens, so an image's repository is built without the weights.
//!
//! `cargo xtask pkgrepo chunks --models DIR [--write]` reads each catalog
//! model's GGUF from `DIR` (the file name the catalog pins), holds it to the
//! pinned length and SHA-256, and makes its list in 2 MiB chunks: it requires
//! the committed list to be byte for byte the same, or writes it with
//! `--write`. The model is only read and hashed, and the GGUFs are large, so
//! this runs on GitHub's runners (model-chunks.yml, release.yml), never on the
//! founder's PC.

use crate::models::catalog::{Catalog, Entry};
use kainet::chunks::{CHUNK, Chunks};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the committed lists lie, from the workspace root.
pub const DIR: &str = "xtask/models/chunks";

/// The committed list's path for the GGUF whose SHA-256 is `sha256` (hex).
pub fn committed(root: &Path, sha256: &str) -> PathBuf {
    root.join(DIR)
        .join(format!("{sha256}{}", kaipkg::models::CHUNKS_EXTENSION))
}

/// `text` as a list for a target of `length` bytes and SHA-256 `sha256` (hex)
/// in P5's 2 MiB chunks, or why not.
pub fn check(text: &str, length: u64, sha256: &str) -> Result<Chunks, String> {
    let list = Chunks::parse(text.as_bytes()).map_err(|why| why.to_string())?;
    if kaipkg::hex(&list.blob) != sha256 || list.size != length {
        return Err(format!(
            "it names another target ({} bytes, SHA-256 {})",
            list.size,
            kaipkg::hex(&list.blob)
        ));
    }
    if list.chunk != CHUNK {
        return Err(format!("its chunk is {} bytes, not {CHUNK}", list.chunk));
    }
    Ok(list)
}

/// The list for the file at `path`, read 2 MiB at a time: the whole file is
/// never in memory.
pub fn of_file(path: &Path) -> Result<Chunks, String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut whole = Sha256::new();
    let mut hashes = Vec::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; CHUNK as usize];
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
            }
        }
        if filled == 0 {
            break;
        }
        let chunk = &buf[..filled];
        whole.update(chunk);
        hashes.push(Sha256::digest(chunk).into());
        size += filled as u64;
        if filled < buf.len() {
            break;
        }
    }
    Ok(Chunks {
        blob: whole.finalize().into(),
        size,
        chunk: CHUNK,
        hashes,
    })
}

/// Checks or writes every catalog model's committed list from the GGUFs in
/// `models`.
pub fn run(root: &Path, models: &Path, write: bool) -> Result<(), String> {
    let catalog = Catalog::load()?;
    let mut wrong = Vec::new();
    for entry in &catalog.models {
        match one(root, models, entry, write) {
            Ok(line) => println!("xtask: {line}"),
            Err(why) => {
                println!("xtask: {}: {why}", entry.id);
                wrong.push(entry.id.clone());
            }
        }
    }
    match wrong.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "the chunk lists of {} are missing or differ; run with --write and commit {DIR}",
            wrong.join(", ")
        )),
    }
}

fn one(root: &Path, models: &Path, entry: &Entry, write: bool) -> Result<String, String> {
    let list = of_file(&models.join(&entry.file))?;
    if list.size != entry.size || kaipkg::hex(&list.blob) != entry.sha256 {
        return Err(format!(
            "{} is not the pinned file ({} bytes, SHA-256 {})",
            entry.file,
            list.size,
            kaipkg::hex(&list.blob)
        ));
    }
    let text = list.render();
    // Read back as `modelchan`'s Job::check_list will: the same blob, size
    // and 2 MiB chunk as the pin, before the list is kept or compared.
    check(&text, entry.size, &entry.sha256)?;
    let path = committed(root, &entry.sha256);
    if write {
        fs::create_dir_all(root.join(DIR)).map_err(|e| e.to_string())?;
        fs::write(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        return Ok(format!(
            "{}: wrote {} ({} chunks)",
            entry.id,
            path.display(),
            list.count()
        ));
    }
    match fs::read_to_string(&path) {
        Ok(kept) if kept == text => Ok(format!(
            "{}: the committed list matches the file ({} chunks of 2 MiB)",
            entry.id,
            list.count()
        )),
        Ok(_) => Err(format!("{} differs from the file's list", path.display())),
        Err(_) => Err(format!("{} is not committed", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_read_in_pieces_gives_the_list_of_its_bytes() {
        let dir = std::env::temp_dir().join(format!("kai-chunks-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for len in [0, 1, CHUNK as usize, 2 * CHUNK as usize + 17] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let path = dir.join(format!("{len}.gguf"));
            fs::write(&path, &bytes).unwrap();
            let list = of_file(&path).unwrap();
            assert_eq!(list, Chunks::of(&bytes, CHUNK), "{len} bytes");
            let sha = crate::sha256_hex(&bytes);
            let checked = check(&list.render(), len as u64, &sha).unwrap();
            assert_eq!(checked, list);
            assert!(check(&list.render(), len as u64 + 1, &sha).is_err());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_list_in_other_chunks_is_refused() {
        let bytes = vec![7u8; 300_000];
        let sha = crate::sha256_hex(&bytes);
        let list = Chunks::of(&bytes, kainet::chunks::MIN_CHUNK).render();
        assert!(
            check(&list, bytes.len() as u64, &sha)
                .unwrap_err()
                .contains("chunk")
        );
    }
}
