//! `import --model` (ADR 0004 §8; P2 task 10): a GGUF file written on the host
//! as the sealed blob `/system/models/<sha256>.gguf`, then named the active
//! model in `kai-modelhint 1`, `kai-data`'s sector 0, which the kernel reads at
//! boot to size the model window.

use super::disk::FileDev;
use super::ops::open;
use kabi::model::Fnv1a;
use kabi::modelhint::Hint;
use kaifs::fs::MODELS;
use kaifs::{BLOCK, BlockDev, Error, ROOT};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The blob's name while it is written and hashed, before its hash names it.
const IMPORTING: &str = ".importing.gguf";
/// Where a model blob keeps its FNV-1a64, little-endian, so `fs` can rewrite
/// the hint after a model change without reading the blob again.
pub const FNV_ATTR: &str = kabi::modelhint::FNV_ATTR;

/// Writes `host` into `image` as a sealed model blob, in one pass that also
/// hashes it, and names it the active model. A model already there is named
/// active again, and not written twice.
pub fn import_model(image: &Path, host: &Path) -> Result<String, String> {
    let shown = host.display().to_string();
    let mut file = File::open(host).map_err(|e| format!("{shown}: {e}"))?;
    let len = file.metadata().map_err(|e| format!("{shown}: {e}"))?.len();
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|e| format!("{shown}: {e}"))?;
    if &magic != b"GGUF" {
        return Err(format!("{shown}: not a GGUF file"));
    }
    let mut file = File::open(host).map_err(|e| format!("{shown}: {e}"))?;
    let mut fs = open(image, true)?;
    fs.set_volume(MODELS)
        .map_err(|e| format!("models: {e:?}"))?;
    let (mut sha, mut fnv) = (Sha256::new(), Fnv1a::new());
    let mut unread = None;
    let made = fs.write_sealed(ROOT, IMPORTING, len, &mut |piece| {
        file.read_exact(piece).map_err(|e| {
            unread = Some(e.to_string());
            Error::Invalid("the host file could not be read")
        })?;
        sha.update(&*piece);
        fnv.update(piece);
        Ok(())
    });
    let ino = match made {
        Ok(ino) => ino,
        Err(e) => {
            let _ = fs.abort();
            return Err(format!("{shown}: {}", unread.unwrap_or(format!("{e:?}"))));
        }
    };
    let sha256: [u8; 32] = sha.finalize().into();
    let fnv = fnv.finish();
    let name: String = sha256.iter().map(|b| format!("{b:02x}")).collect();
    let name = format!("{name}.gguf");
    let path = format!("/system/models/{name}");
    let fail = |e: Error| format!("{path}: {e:?}");
    let line = match fs.lookup(ROOT, &name).map_err(fail)? {
        Some(_) => {
            fs.abort().map_err(fail)?;
            format!("{path} was there already: named the active model")
        }
        None => {
            fs.rename(ROOT, IMPORTING, ROOT, &name).map_err(fail)?;
            fs.set_xattr(ino, FNV_ATTR, &fnv.to_le_bytes())
                .map_err(fail)?;
            fs.clock = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64);
            let generation = fs.commit().map_err(fail)?;
            format!("{path}: {len} bytes, sealed, in generation {generation}; the active model")
        }
    };
    drop(fs);
    // After the commit, as `fs` does after a model change: a crash between the
    // two leaves the old hint, and the kernel loads the old model.
    let mut dev = FileDev::open(image, true)?;
    let mut block = [0u8; BLOCK];
    let hint = Hint { len, fnv, sha256 }.encode();
    block[..hint.len()].copy_from_slice(&hint);
    dev.write(0, &block)
        .and_then(|_| dev.flush())
        .map_err(|_| format!("{}: writing kai-data's sector 0", image.display()))?;
    Ok(line)
}
