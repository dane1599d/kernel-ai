//! `cargo xtask release-repo prepare | prepare-timestamp | assemble | check`
//! (P4 task 18's interim path, `docs/release/root-ceremony.md`, "Interim: key
//! files"): the image repository a release ships, `release/pkgrepo/`, signed
//! with the founder's two Ed25519 key files, `root.key` for root and
//! `release.key` for every other role. xtask never reads either: it reads
//! only their public halves, `release/keys/root.pub` and `release.pub`.
//!
//! 1. `prepare` writes each role's canonical `signed` bytes as `<role>.canon`
//!    in the work folder (`target/release-repo`): targets, packages, models
//!    and snapshot, at version `release_seq`, and root at version 1.
//!    `packages` lists the built-in apps packed from this build's components,
//!    whose bytes it writes beside the role files (`packages.rs`).
//!    `models` lists [`models::catalog`], each model with its committed chunk
//!    list, never a test fixture. A committed root is frozen: it is reused,
//!    never signed again, and it must name the same two keys.
//! 2. The founder runs `scripts/release-sign.sh`, which writes `<role>.sig`
//!    beside each, the raw Ed25519 signature in hex (`openssl pkeyutl`).
//! 3. `prepare-timestamp` checks the snapshot's signature and writes
//!    `timestamp.canon`. It holds the signed snapshot's length and SHA-256, so
//!    it is signed in a second round.
//! 4. `assemble` checks each signature with kaituf, then the whole repository
//!    as a device does (`kaipkg::image`), and writes `release/pkgrepo/metadata/`.
//!
//! `check` makes the last step's checks on what is committed. `release-image`
//! makes them before it bundles the repository as `pkgrepo/`, and its root as
//! `kai.root.json`.

use crate::pkgrepo::models::{self, Model};
use crate::pkgrepo::tuf;
use check::{Mode, check_repository, read_repository};
use kaipkg::generation::Time;
use kaipkg::image;
use kaipkg::sign::Target;
use kaipkg::sign::{self, ADR_0008, MODELS, PACKAGES, PublicKey, Roles};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

mod check;
pub mod packages;
#[cfg(test)]
mod tests;

pub use check::{RepoFiles, committed};

/// The founder's public keys, under the workspace.
pub const KEYS: &str = "release/keys";
/// The signed repository a release image bundles, under the workspace.
pub const REPO: &str = "release/pkgrepo";
/// Where `prepare` writes, under the workspace, unless `--dir` says.
const WORK: &str = "target/release-repo";
/// The DER prefix of an Ed25519 SubjectPublicKeyInfo, before its 32 bytes:
/// SEQUENCE { SEQUENCE { id-Ed25519 }, BIT STRING }.
const ED25519_SPKI: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
/// The roles of the first round, after root when it is not frozen.
const FIRST_ROUND: [&str; 4] = ["targets", PACKAGES, MODELS, "snapshot"];
/// Every role a release signs: the only `.canon` and `.sig` names there are.
const ROLES: [&str; 6] = ["root", "targets", PACKAGES, MODELS, "snapshot", "timestamp"];

const USAGE: &str =
    "usage: cargo xtask release-repo prepare|prepare-timestamp|assemble [--dir DIR] | check";

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let work = match rest {
        [] => root.join(WORK),
        [flag, dir] if flag == "--dir" && command != "check" => PathBuf::from(dir),
        _ => return Err(USAGE.into()),
    };
    let paths = Paths::of(&root, work);
    let now = crate::pkgrepo::unix_now()?;
    match command.as_str() {
        "prepare" => {
            let version = crate::release_image::release_seq(&root)?.into();
            let versions = crate::components::build(&root)?;
            let targets = packages::pack(&root, &versions)?;
            prepare(&paths, &targets, &models::catalog()?, version, now)?;
            println!(
                "xtask: sign them: release-sign.sh <key folder> {}",
                paths.work.display()
            );
            println!("xtask: then: cargo xtask release-repo prepare-timestamp");
            Ok(())
        }
        "prepare-timestamp" => {
            prepare_timestamp(&paths, now)?;
            println!(
                "xtask: sign it: release-sign.sh <key folder> {}",
                paths.work.display()
            );
            println!("xtask: then: cargo xtask release-repo assemble");
            Ok(())
        }
        "assemble" => {
            let version = crate::release_image::release_seq(&root)?.into();
            assemble(&paths, &models::catalog()?, version, now)?;
            println!("xtask: wrote {} (commit it)", paths.repo.display());
            Ok(())
        }
        // Hash-only: the packages are not built here (`ci.yml`).
        "check" => match committed(&root, None)? {
            Some(files) => {
                println!("xtask: {REPO} verifies: {} files", files.len());
                Ok(())
            }
            None => Err(format!("{REPO} has no {}", image::ROOT)),
        },
        _ => Err(USAGE.into()),
    }
}

/// Where a release's public keys, signed repository and work folder are.
struct Paths {
    keys: PathBuf,
    repo: PathBuf,
    work: PathBuf,
}

impl Paths {
    fn of(root: &Path, work: PathBuf) -> Self {
        Paths {
            keys: root.join(KEYS),
            repo: root.join(REPO),
            work,
        }
    }
}

/// The founder's two public keys.
struct FileKeys {
    root: PublicKey,
    release: PublicKey,
}

impl FileKeys {
    fn read(dir: &Path) -> Result<Self, String> {
        let key = |name: &str| {
            let path = dir.join(name);
            let pem = fs::read_to_string(&path).map_err(|e| {
                format!(
                    "cannot read {}: {e} (the founder's public keys come first)",
                    path.display()
                )
            })?;
            read_public(&pem).map_err(|why| format!("{}: {why}", path.display()))
        };
        Ok(FileKeys {
            root: key("root.pub")?,
            release: key("release.pub")?,
        })
    }

    /// The keys each role names: root's for root, the release key for the rest.
    fn roles(&self) -> Roles {
        let release = self.release;
        Roles {
            root: self.root,
            targets: release,
            snapshot: release,
            timestamp: release,
            packages: release,
            models: Some(release),
        }
    }

    fn signer(&self, role: &str) -> PublicKey {
        match role {
            "root" => self.root,
            _ => self.release,
        }
    }
}

/// An Ed25519 public key as `openssl pkey -pubout` writes it: a PEM
/// SubjectPublicKeyInfo, whose DER is [`ED25519_SPKI`] then the key's 32
/// bytes, the form kaituf's metadata names in hex.
pub fn read_public(pem: &str) -> Result<PublicKey, String> {
    let body = pem
        .trim()
        .strip_prefix("-----BEGIN PUBLIC KEY-----")
        .and_then(|rest| rest.strip_suffix("-----END PUBLIC KEY-----"))
        .ok_or("not a PEM public key")?;
    let der = kaituf::keys::base64(body).ok_or("its body is not base64")?;
    der.strip_prefix(&ED25519_SPKI[..])
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .map(PublicKey)
        .ok_or_else(|| "not an Ed25519 public key".into())
}

/// The roles of the release repository for `targets`, the built-in apps'
/// packages, and `models` at `version`, each role's canonical bytes written
/// to the work folder as `<role>.canon`, the committed root's excepted, and
/// the packages' bytes beside them. Old `.canon` and `.sig` files go first.
fn prepare(
    paths: &Paths,
    targets: &[Target],
    models: &[Model],
    version: u64,
    now: Time,
) -> Result<(), String> {
    let keys = FileKeys::read(&paths.keys)?;
    if let Some(model) = models.iter().find(|m| m.chunks.is_none()) {
        return Err(format!(
            "{} has no committed chunk list in xtask/models/chunks: every catalog model needs one",
            model.id
        ));
    }
    let plan = plan(&keys, targets, models, version, now)?;
    clear(&paths.work)?;
    let mut roles = Vec::new();
    match frozen_root(paths, &keys, &plan.root)? {
        Some(_) => println!(
            "xtask: {REPO}/{} is committed and frozen: root is not signed again",
            image::ROOT
        ),
        None => roles.push(("root", &plan.root)),
    }
    let models_role = plan.models.as_ref().ok_or("the plan has no models role")?;
    let signed = [&plan.targets, &plan.packages, models_role, &plan.snapshot];
    roles.extend(FIRST_ROUND.into_iter().zip(signed));
    for (role, signed) in roles {
        write_canon(&paths.work, role, signed)?;
    }
    packages::write(&paths.work, &packages::built(targets))
}

/// The roles a release signs: `targets`, the built-in apps' packages, and
/// `models`, named by `keys`.
fn plan(
    keys: &FileKeys,
    targets: &[Target],
    models: &[Model],
    version: u64,
    now: Time,
) -> Result<sign::Unsigned, String> {
    let models = tuf::sign_models(models)?;
    sign::unsigned(targets, &models, &keys.roles(), now, ADR_0008, version)
}

/// The committed root, if there is one, once it is checked to be `root` but
/// for its expiry and to carry root.key's signature. It is never signed again:
/// `updated` keeps a cached root of the same version.
fn frozen_root(paths: &Paths, keys: &FileKeys, root: &Value) -> Result<Option<Vec<u8>>, String> {
    let path = paths.repo.join(image::ROOT);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let committed: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if !check::same_but_expiry(&committed["signed"], root) {
        return Err(format!(
            "{} is not the root {KEYS} names: the committed root is frozen",
            path.display()
        ));
    }
    check::root_signed(&[(image::ROOT.into(), bytes.clone())], keys)?;
    Ok(Some(bytes))
}

/// The timestamp for the signed snapshot, written as `timestamp.canon`.
fn prepare_timestamp(paths: &Paths, now: Time) -> Result<(), String> {
    let keys = FileKeys::read(&paths.keys)?;
    let (snapshot, version) = signed_file(&paths.work, "snapshot", &keys)?;
    let _ = fs::remove_file(paths.work.join("timestamp.sig"));
    write_canon(
        &paths.work,
        "timestamp",
        &sign::timestamp(&snapshot, version, now, ADR_0008),
    )
}

/// Every role signed, checked at `now` as [`check_repository`] does, the
/// non-root roles at `version`, and written to the committed repository.
fn assemble(paths: &Paths, models: &[Model], version: u64, now: Time) -> Result<(), String> {
    let keys = FileKeys::read(&paths.keys)?;
    let (root, fresh) = match fs::read(paths.repo.join(image::ROOT)) {
        Ok(bytes) => (bytes, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (signed_file(&paths.work, "root", &keys)?.0, true)
        }
        Err(e) => return Err(format!("cannot read the committed root: {e}")),
    };
    let mut files = vec![(image::ROOT.to_string(), root)];
    for role in FIRST_ROUND {
        let (bytes, version) = signed_file(&paths.work, role, &keys)?;
        files.push((sign::path(role, version), bytes));
    }
    let (timestamp, _) = signed_file(&paths.work, "timestamp", &keys)?;
    files.push((sign::TIMESTAMP_PATH.into(), timestamp));
    let built = packages::read(&paths.work)?;
    let mode = Mode::Signed { version, fresh };
    check_repository(&files, models, Some(&built), &keys, now, mode)?;
    write_repository(&paths.repo, &files)
}

/// `<role>.canon` with its `<role>.sig`, once the signature is checked with
/// kaituf against the key that signs `role`: the role's file, and its version.
fn signed_file(work: &Path, role: &str, keys: &FileKeys) -> Result<(Vec<u8>, u64), String> {
    let read =
        |name: String| fs::read(work.join(&name)).map_err(|e| format!("cannot read {name}: {e}"));
    let canon = read(format!("{role}.canon"))?;
    let signed: Value = serde_json::from_slice(&canon).map_err(|e| format!("{role}.canon: {e}"))?;
    if sign::canonical(&signed)? != canon {
        return Err(format!("{role}.canon is not canonical JSON"));
    }
    let text = String::from_utf8(read(format!("{role}.sig"))?).unwrap_or_default();
    let sig = kaituf::unhex(text.trim())
        .and_then(|raw| <[u8; 64]>::try_from(raw).ok())
        .ok_or_else(|| format!("{role}.sig is not 128 hex digits"))?;
    let key = keys.signer(role);
    let verifier = kaituf::keys::Key::parse(&key.public()).map_err(|e| e.to_string())?;
    if !verifier.verifies(&canon, text.trim()) {
        let file = if role == "root" {
            "root.key"
        } else {
            "release.key"
        };
        return Err(format!(
            "{role}.sig is not {file}'s signature of {role}.canon"
        ));
    }
    let version = signed["version"]
        .as_u64()
        .ok_or_else(|| format!("{role}.canon has no version"))?;
    Ok((sign::signed_envelope(&signed, &key, &sig)?, version))
}

/// `files` as `repo`'s `metadata/`: every other role file there goes, and a
/// committed root must be `files`' byte for byte. Every root file is frozen,
/// so a later root committed beside version 1 stops it: this signs version 1
/// only, and the YubiKey root's tools are still to come.
fn write_repository(repo: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let dir = repo.join(image::METADATA);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let committed = read_repository(repo)?;
    if let Some((path, _)) = committed
        .iter()
        .find(|(p, _)| p.ends_with(".root.json") && p != image::ROOT)
    {
        return Err(format!(
            "{path} is committed: every root file is frozen, and this signs root version 1 only"
        ));
    }
    for (path, _) in committed {
        if path != image::ROOT {
            fs::remove_file(repo.join(&path)).map_err(|e| format!("cannot remove {path}: {e}"))?;
        }
    }
    for (path, bytes) in files {
        let at = repo.join(path);
        if path == image::ROOT && at.exists() {
            match fs::read(&at) {
                Ok(committed) if committed == *bytes => continue,
                _ => return Err(format!("{} is frozen and differs", at.display())),
            }
        }
        fs::write(&at, bytes).map_err(|e| format!("cannot write {}: {e}", at.display()))?;
    }
    Ok(())
}

/// Makes `work`, and removes the role files an earlier run left there: each
/// `<role>.canon` and `<role>.sig` of [`ROLES`], and nothing else.
fn clear(work: &Path) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("cannot make {}: {e}", work.display()))?;
    for role in ROLES {
        for file in [format!("{role}.canon"), format!("{role}.sig")] {
            let path = work.join(file);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove {}: {e}", path.display())),
            }
        }
    }
    Ok(())
}

/// `signed`'s canonical bytes as `<role>.canon`, with what they hash to.
fn write_canon(work: &Path, role: &str, signed: &Value) -> Result<(), String> {
    let bytes = sign::canonical(signed)?;
    let path = work.join(format!("{role}.canon"));
    fs::write(&path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let version = &signed["version"];
    println!(
        "xtask: {role}.canon, version {version}, {} bytes, sha256 {}",
        bytes.len(),
        crate::sha256_hex(&bytes)
    );
    Ok(())
}
