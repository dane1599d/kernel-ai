//! Downloads a catalog model with curl, which draws a progress bar and resumes a
//! partial `.part` file, then checks its size and SHA-256 before the file gets
//! its final name. A file that does not match is deleted, never kept.

use super::catalog::Entry;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const CHUNK: usize = 8 << 20;

/// The store path of `entry`'s file, downloading and checking it if needed.
pub fn fetch(entry: &Entry, store: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(store).map_err(|e| format!("cannot create {}: {e}", store.display()))?;
    let target = store.join(&entry.file);
    if target.exists() {
        println!("xtask: {} is already in the store; checking it", entry.file);
        return verify(&target, entry)
            .map(|()| target.clone())
            .map_err(|e| format!("{e}; delete {} and run setup again", target.display()));
    }
    let part = store.join(format!("{}.part", entry.file));
    let mut have = fs::metadata(&part).map_or(0, |m| m.len());
    if have > entry.size {
        fs::remove_file(&part).map_err(|e| format!("cannot remove {}: {e}", part.display()))?;
        have = 0;
    }
    if have < entry.size {
        if have > 0 {
            println!(
                "xtask: resuming at {} of {} MB",
                have / 1_000_000,
                entry.size / 1_000_000
            );
        }
        let status = Command::new("curl")
            .args([
                "-fL",
                "--retry",
                "5",
                "--retry-delay",
                "2",
                "-C",
                "-",
                "--progress-bar",
            ])
            .arg("-o")
            .arg(&part)
            .arg(entry.url())
            .status()
            .map_err(|e| format!("cannot run curl: {e}"))?;
        if !status.success() {
            return Err(format!(
                "curl failed ({status}); run the same command again to resume from {}",
                part.display()
            ));
        }
    }
    finish(&part, &target, entry)?;
    Ok(target)
}

/// Gives a complete `.part` file its final name if it is the pinned file, and
/// deletes it if it is not.
pub fn finish(part: &Path, target: &Path, entry: &Entry) -> Result<(), String> {
    if let Err(e) = verify(part, entry) {
        return Err(match fs::remove_file(part) {
            Ok(()) => format!("{e}; the download was deleted"),
            Err(why) => format!("{e}; delete {} by hand ({why})", part.display()),
        });
    }
    fs::rename(part, target).map_err(|e| {
        format!(
            "cannot rename {} to {}: {e}",
            part.display(),
            target.display()
        )
    })
}

/// Checks size, then SHA-256, against the catalog.
pub fn verify(path: &Path, entry: &Entry) -> Result<(), String> {
    let len = fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    if len != entry.size {
        return Err(format!(
            "{} has {len} bytes, but the catalog pins {}",
            path.display(),
            entry.size
        ));
    }
    let actual = sha256_file(path)?;
    if actual != entry.sha256 {
        return Err(format!(
            "{} has SHA-256 {actual}, but the catalog pins {}",
            path.display(),
            entry.sha256
        ));
    }
    Ok(())
}

/// The file's SHA-256 in lowercase hex, with a percentage on one line.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let fail = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    let mut file = fs::File::open(path).map_err(fail)?;
    let total = file.metadata().map_err(fail)?.len().max(1);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let (mut done, mut shown) = (0u64, 0);
    loop {
        let n = file.read(&mut buf).map_err(fail)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        let percent = done * 100 / total;
        if total > CHUNK as u64 && percent >= shown + 10 {
            shown = percent;
            print!("\rxtask: checking SHA-256 {percent}%");
            let _ = std::io::stdout().flush();
        }
    }
    if shown > 0 {
        println!();
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(bytes: &[u8]) -> Entry {
        let mut entry = super::super::catalog::Catalog::load().unwrap().models[0].clone();
        entry.size = bytes.len() as u64;
        entry.sha256 = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        entry
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kai-fetch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_pinned_file_gets_its_name() {
        let dir = scratch("good");
        let (part, target) = (dir.join("m.gguf.part"), dir.join("m.gguf"));
        fs::write(&part, b"GGUF model bytes").unwrap();
        finish(&part, &target, &entry_for(b"GGUF model bytes")).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"GGUF model bytes");
        assert!(!part.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_tampered_download_is_rejected_and_deleted() {
        let dir = scratch("tampered");
        let (part, target) = (dir.join("m.gguf.part"), dir.join("m.gguf"));
        let entry = entry_for(b"GGUF model bytes");
        fs::write(&part, b"GGUF model bytez").unwrap();
        let error = finish(&part, &target, &entry).unwrap_err();
        assert!(
            error.contains("SHA-256") && error.contains("deleted"),
            "{error}"
        );
        assert!(!part.exists() && !target.exists());
        fs::write(&part, b"GGUF model").unwrap();
        let error = finish(&part, &target, &entry).unwrap_err();
        assert!(error.contains("bytes"), "{error}");
        assert!(!part.exists() && !target.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sha256_streams_large_files() {
        let dir = scratch("hash");
        let path = dir.join("big");
        let data: Vec<u8> = (0..(CHUNK + 1000)).map(|i| i as u8).collect();
        fs::write(&path, &data).unwrap();
        let expected: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha256_file(&path).unwrap(), expected);
        fs::remove_dir_all(dir).unwrap();
    }
}
