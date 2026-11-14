//! The built-in apps a release signs as packages (founder test 2; the
//! manager's option (b)). The shell builds an app's rows from installed
//! packages, so a release that signed none would know no app's actions until
//! the app ran.
//!
//! The founder signs each package's length and SHA-256, so the bytes signed
//! must be the bytes every build of that commit packs. `release-prepare.yml`
//! packs them on two runners and refuses unless both agree, then `prepare`
//! writes them beside the role files, `packages/<name>-<version>.kpkg` with a
//! `SHA256SUMS`. `assemble` holds the signed role to those files, and
//! `release-image` to what it packs itself. A difference names the package and
//! both hashes, and nothing is packed again to fit.

use crate::esp::Module;
use crate::pkgrepo::{self, tuf};
use kaipkg::sign::Target;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Each package by its target path, `<name>-<version>.kpkg`, and its bytes.
pub type Built = Vec<(String, Vec<u8>)>;

/// Where `prepare` writes the packages it signed, in its work folder.
pub const DIR: &str = "packages";
/// Their SHA-256s, as `sha256sum` writes them.
const SUMS: &str = "SHA256SUMS";
/// A package target's ending.
const EXTENSION: &str = ".kpkg";

/// The built-in apps packed from `versions`, this build's components, as the
/// `packages` role lists them.
pub fn pack(root: &Path, versions: &[Module]) -> Result<Vec<Target>, String> {
    Ok(tuf::sign_targets(&pkgrepo::pack_all(root, versions)?))
}

/// `targets`' paths and bytes.
pub fn built(targets: &[Target]) -> Built {
    targets
        .iter()
        .map(|t| (t.path.clone(), t.bytes.clone()))
        .collect()
}

/// `built` as `<work>/packages/`, after whatever an earlier run left there.
pub fn write(work: &Path, built: &[(String, Vec<u8>)]) -> Result<(), String> {
    let dir = work.join(DIR);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let mut sums = String::new();
    for (path, bytes) in built {
        fs::write(dir.join(path), bytes).map_err(|e| format!("cannot write {path}: {e}"))?;
        sums.push_str(&format!("{}  {path}\n", crate::sha256_hex(bytes)));
        println!(
            "xtask: package {path}, {} bytes, sha256 {}",
            bytes.len(),
            crate::sha256_hex(bytes)
        );
    }
    fs::write(dir.join(SUMS), sums).map_err(|e| format!("cannot write {SUMS}: {e}"))
}

/// What [`write`] left in `work`, in name order.
pub fn read(work: &Path) -> Result<Built, String> {
    let dir = work.join(DIR);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(EXTENSION))
        .collect();
    names.sort();
    let file = |name: String| {
        let bytes = fs::read(dir.join(&name)).map_err(|e| format!("cannot read {name}: {e}"))?;
        Ok((name, bytes))
    };
    names.into_iter().map(file).collect()
}

/// The `packages` role's `targets`, each a `<name>-<version>.kpkg` with a
/// length, a SHA-256 and the `custom.kai` index that names it so. With
/// `built`, they are exactly its packages, each of its length and SHA-256.
pub fn check(
    listed: &Value,
    built: Option<&[(String, Vec<u8>)]>,
    repo: &str,
) -> Result<(), String> {
    let listed = listed
        .as_object()
        .ok_or_else(|| format!("{repo}: the packages role lists no targets"))?;
    for (path, target) in listed {
        let kai = &target["custom"]["kai"];
        let named = format!(
            "{}-{}{}",
            kai["name"].as_str().unwrap_or_default(),
            kai["version"].as_str().unwrap_or_default(),
            EXTENSION
        );
        if *path != named || signed(target).is_none() {
            return Err(format!(
                "{repo}: the packages role's {path} is not a package's target"
            ));
        }
    }
    let Some(built) = built else {
        return Ok(());
    };
    for (path, bytes) in built {
        let packed = (bytes.len() as u64, crate::sha256_hex(bytes));
        match listed.get(path).and_then(signed) {
            None => {
                return Err(format!(
                    "this build packs {path}, which {repo} does not sign: sign the packages again"
                ));
            }
            Some(signed) if signed != packed => {
                return Err(format!(
                    "{path}: {repo} signs {} bytes, sha256 {}; this build packs {} bytes, sha256 {}: \
                     sign the packages again (docs/release/root-ceremony.md)",
                    signed.0, signed.1, packed.0, packed.1
                ));
            }
            Some(_) => {}
        }
    }
    match listed
        .keys()
        .find(|path| !built.iter().any(|(p, _)| p == *path))
    {
        Some(path) => Err(format!(
            "{repo} signs {path}, which this build does not pack: sign the packages again"
        )),
        None => Ok(()),
    }
}

/// A listed target's length and SHA-256, if it has both.
fn signed(target: &Value) -> Option<(u64, String)> {
    let sha = target["hashes"]["sha256"]
        .as_str()
        .filter(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some((target["length"].as_u64()?, sha.to_string()))
}
