//! The built-in apps a release signs as packages (founder test 2; the
//! manager's option (b)). The shell builds an app's rows from installed
//! packages, so a release that signed none would know no app's actions until
//! the app ran.
//!
//! The founder signs each package's whole entry: its length and SHA-256, and
//! its `custom.kai` index, whose grants, actions and effects the OS acts on.
//! So the bytes signed must be the bytes every build of that commit packs, and
//! the index the one those bytes make. `release-prepare.yml` packs them on two
//! runners and refuses unless both agree, then `prepare` writes them beside
//! the role files, `packages/<name>-<version>.kpkg` with a `SHA256SUMS`.
//! `assemble` packs each again from the artifact inside it ([`targets`]), and
//! `release-image` from its own build; each holds the signed role to exactly
//! the entries `prepare` writes for those ([`check`]). A difference names the
//! package, and nothing is packed again to fit.

use crate::esp::Module;
use crate::pkgrepo::{self, tuf};
use kaipkg::sign::{self, Target};
use serde_json::Value;
use std::collections::BTreeSet;
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
/// The fields a package's `custom.kai` index may have
/// (`pkgrepo::Package::index`).
const INDEX_FIELDS: [&str; 11] = [
    "actions", "depends", "effects", "grants", "kind", "licence", "name", "summary", "version",
    "world", "yanked",
];

/// The built-in apps packed from `versions`, this build's components, as the
/// `packages` role lists them.
pub fn pack(root: &Path, versions: &[Module]) -> Result<Vec<Target>, String> {
    Ok(tuf::sign_targets(&pkgrepo::pack_all(root, versions)?))
}

/// The `packages` role's targets for `built`, as `prepare` lists them: each
/// package packed again from the artifact inside it, which must give its very
/// bytes. So its index is the one its own artifact makes, never one read from
/// a file.
pub fn targets(root: &Path, built: &[(String, Vec<u8>)]) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();
    for (path, bytes) in built {
        let files = kaipkg::bundle::files(bytes).map_err(|why| format!("{path}: {why}"))?;
        let artifact = files
            .iter()
            .find(|(name, _)| *name == pkgrepo::ARTIFACT)
            .map(|(_, artifact)| *artifact)
            .ok_or_else(|| format!("{path} holds no artifact"))?;
        let apps = pkgrepo::APPS.iter().filter(|app| {
            path.strip_prefix(app.name)
                .is_some_and(|v| v.starts_with('-'))
        });
        let mut packed = None;
        for app in apps {
            let package = pkgrepo::pack(root, app, artifact)?;
            if package.target() == *path {
                packed = Some(package);
                break;
            }
        }
        let package = packed.ok_or_else(|| format!("{path} is no built-in app's package"))?;
        if package.bytes != *bytes {
            return Err(format!(
                "{path} packs again to sha256 {}, not {}: assemble at the commit prepare ran at",
                crate::sha256_hex(&package.bytes),
                crate::sha256_hex(bytes)
            ));
        }
        targets.extend(tuf::sign_targets(std::slice::from_ref(&package)));
    }
    Ok(targets)
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

/// The `packages` role's `targets`. Each entry has exactly the shape
/// `kaipkg::sign::target_entry` writes: a length, a SHA-256, and a
/// `custom.kai` index that names it and is not yanked. With `built`, they are
/// exactly its packages, each entry equal to the one `prepare` writes for it,
/// grants, actions and effects included.
pub fn check(listed: &Value, built: Option<&[Target]>, repo: &str) -> Result<(), String> {
    let listed = listed
        .as_object()
        .ok_or_else(|| format!("{repo}: the packages role lists no targets"))?;
    for (path, entry) in listed {
        shape(path, entry).map_err(|why| format!("{repo}: the packages role's {path} {why}"))?;
    }
    let Some(built) = built else {
        return Ok(());
    };
    for target in built {
        let path = &target.path;
        let packed = sign::target_entry(target);
        match listed.get(path) {
            None => {
                return Err(format!(
                    "this build packs {path}, which {repo} does not sign: sign the packages again"
                ));
            }
            Some(signed) if *signed != packed => {
                return Err(format!(
                    "{path}: {}: sign the packages again (docs/release/root-ceremony.md)",
                    differs(signed, &packed, repo)
                ));
            }
            Some(_) => {}
        }
    }
    match listed
        .keys()
        .find(|path| !built.iter().any(|t| t.path == **path))
    {
        Some(path) => Err(format!(
            "{repo} signs {path}, which this build does not pack: sign the packages again"
        )),
        None => Ok(()),
    }
}

/// Why `entry` is not shaped as `prepare` writes a package's, if it is not.
fn shape(path: &str, entry: &Value) -> Result<(), &'static str> {
    fn fields(value: &Value) -> Option<Vec<&str>> {
        let mut keys: Vec<&str> = value.as_object()?.keys().map(String::as_str).collect();
        keys.sort_unstable();
        Some(keys)
    }
    if fields(entry) != Some(vec!["custom", "hashes", "length"]) {
        return Err("has fields other than custom, hashes and length");
    }
    if fields(&entry["hashes"]) != Some(vec!["sha256"]) {
        return Err("has hashes other than one SHA-256");
    }
    let sha = entry["hashes"]["sha256"].as_str().unwrap_or_default();
    if sha.len() != 64 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err("has no SHA-256 in hex");
    }
    if entry["length"].as_u64().is_none() {
        return Err("has no length");
    }
    if fields(&entry["custom"]) != Some(vec!["kai"]) {
        return Err("has custom fields other than kai");
    }
    let kai = &entry["custom"]["kai"];
    if !fields(kai).is_some_and(|keys| keys.iter().all(|k| INDEX_FIELDS.contains(k))) {
        return Err("has index fields a package's index has not");
    }
    let named = format!(
        "{}-{}{EXTENSION}",
        kai["name"].as_str().unwrap_or_default(),
        kai["version"].as_str().unwrap_or_default(),
    );
    if path != named {
        return Err("is not the name and version its index gives");
    }
    if kai.get("yanked").is_some_and(|yanked| *yanked != false) {
        return Err("is yanked");
    }
    Ok(())
}

/// How a signed entry differs from the one this build packs: its hash, or
/// each index field that differs, as signed and as packed.
fn differs(signed: &Value, packed: &Value, repo: &str) -> String {
    let hash = |entry: &Value| (entry["length"].clone(), entry["hashes"]["sha256"].clone());
    let ((length, sha), (built_length, built_sha)) = (hash(signed), hash(packed));
    if (&length, &sha) != (&built_length, &built_sha) {
        return format!(
            "{repo} signs {length} bytes, sha256 {}; this build packs {built_length} bytes, sha256 {}",
            sha.as_str().unwrap_or_default(),
            built_sha.as_str().unwrap_or_default()
        );
    }
    let (signed, packed) = (&signed["custom"]["kai"], &packed["custom"]["kai"]);
    let fields: BTreeSet<&String> = [signed, packed]
        .iter()
        .filter_map(|kai| kai.as_object())
        .flat_map(|kai| kai.keys())
        .collect();
    let shown = |value: Option<&Value>| value.map_or_else(|| "nothing".into(), Value::to_string);
    let changed: Vec<String> = fields
        .into_iter()
        .filter(|field| signed.get(field) != packed.get(field))
        .map(|field| {
            format!(
                "{field} {} where this build has {}",
                shown(signed.get(field)),
                shown(packed.get(field))
            )
        })
        .collect();
    format!("{repo} signs another index: {}", changed.join("; "))
}
