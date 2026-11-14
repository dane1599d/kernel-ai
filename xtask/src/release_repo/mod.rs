//! `cargo xtask release-repo prepare | prepare-timestamp | assemble | check`
//! (P4 task 18's interim path, `docs/release/root-ceremony.md`, "Interim: key
//! files"): the image repository a release ships, `release/pkgrepo/`, signed
//! with the founder's two Ed25519 key files, `root.key` for root and
//! `release.key` for every other role. xtask never reads either: it reads
//! only their public halves, `release/keys/root.pub` and `release.pub`.
//!
//! 1. `prepare` writes each role's canonical `signed` bytes as `<role>.canon`
//!    in the work folder (`target/release-repo`): targets, packages (empty),
//!    models and snapshot, at version `release_seq`, and root at version 1.
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
use kaipkg::generation::Time;
use kaipkg::image::{self, Files};
use kaipkg::sign::{self, ADR_0008, MODELS, PACKAGES, PublicKey, Roles};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

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
            prepare(&paths, &models::catalog()?, version, now)?;
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
            assemble(&paths, &models::catalog()?)?;
            println!("xtask: wrote {} (commit it)", paths.repo.display());
            Ok(())
        }
        "check" => match committed(&root)? {
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

/// The roles of the release repository for `models` at `version`, each
/// role's canonical bytes written to the work folder as `<role>.canon`, the
/// committed root's excepted. Old `.canon` and `.sig` files go first.
fn prepare(paths: &Paths, models: &[Model], version: u64, now: Time) -> Result<(), String> {
    let keys = FileKeys::read(&paths.keys)?;
    if let Some(model) = models.iter().find(|m| m.chunks.is_none()) {
        return Err(format!(
            "{} has no committed chunk list in xtask/models/chunks: every catalog model needs one",
            model.id
        ));
    }
    let plan = plan(&keys, models, version, now)?;
    clear(&paths.work)?;
    let mut roles = Vec::new();
    match frozen_root(paths, &plan.root)? {
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
    Ok(())
}

/// The roles a release signs: `models` and no package, named by `keys`.
fn plan(
    keys: &FileKeys,
    models: &[Model],
    version: u64,
    now: Time,
) -> Result<sign::Unsigned, String> {
    let models = tuf::sign_models(models)?;
    sign::unsigned(&[], &models, &keys.roles(), now, ADR_0008, version)
}

/// The committed root, if there is one, once it is checked to name the keys
/// and roles `root` names: whatever else it holds, such as its expiry, it is
/// never signed again (`updated` keeps a cached root of the same version).
fn frozen_root(paths: &Paths, root: &Value) -> Result<Option<Vec<u8>>, String> {
    let path = paths.repo.join(image::ROOT);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let committed: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let signed = &committed["signed"];
    let same = ["keys", "roles", "consistent_snapshot", "version"]
        .iter()
        .all(|field| signed[field] == root[field]);
    match same {
        true => Ok(Some(bytes)),
        false => Err(format!(
            "{} names other keys or roles than {KEYS}: the committed root is frozen",
            path.display()
        )),
    }
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

/// Every role signed, checked and written to the committed repository.
fn assemble(paths: &Paths, models: &[Model]) -> Result<(), String> {
    let keys = FileKeys::read(&paths.keys)?;
    let root = match fs::read(paths.repo.join(image::ROOT)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            signed_file(&paths.work, "root", &keys)?.0
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
    check_repository(&files, models, &keys)?;
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

/// A repository's files, by path from its top.
pub type RepoFiles = Vec<(String, Vec<u8>)>;

/// The committed repository, checked as [`check_repository`] does, or none
/// when nothing is committed.
pub fn committed(root: &Path) -> Result<Option<RepoFiles>, String> {
    let repo = root.join(REPO);
    if !repo.join(image::ROOT).exists() {
        return Ok(None);
    }
    let keys = FileKeys::read(&root.join(KEYS))?;
    let files = read_repository(&repo)?;
    check_repository(&files, &models::catalog()?, &keys)?;
    Ok(Some(files))
}

/// The repository as a device checks the image's (`kaipkg::image::listed`,
/// expiry not checked), then against what a release signs: a root that names
/// `keys`, no package, and the `models` role's targets exactly `models`'.
fn check_repository(
    files: &[(String, Vec<u8>)],
    models: &[Model],
    keys: &FileKeys,
) -> Result<(), String> {
    let mut repository = InMemory(files);
    let listed = image::listed(&mut repository, MODELS).map_err(|why| format!("{REPO}: {why}"))?;
    if let Some((path, why)) = listed.refused.first() {
        return Err(format!("{REPO}: {path} is refused: {why}"));
    }
    if !image::listed(&mut repository, PACKAGES)?.targets.is_empty() {
        return Err(format!("{REPO}: a release's packages role lists nothing"));
    }
    let plan = plan(keys, models, 1, Time(0))?;
    let root: Value = parse(files, image::ROOT)?;
    if root["signed"]["keys"] != plan.root["keys"] || root["signed"]["roles"] != plan.root["roles"]
    {
        return Err(format!("{REPO}: its root names other keys than {KEYS}"));
    }
    let want = plan.models.as_ref().map(|m| &m["targets"]);
    if Some(&role_file(files, MODELS)?["signed"]["targets"]) != want {
        return Err(format!(
            "{REPO}: its models role is not xtask/models.json's catalog: sign it again"
        ));
    }
    Ok(())
}

/// The role file the timestamp leads to, through the snapshot.
fn role_file(files: &[(String, Vec<u8>)], role: &str) -> Result<Value, String> {
    let version = |meta: &Value, name: &str| {
        meta["signed"]["meta"][name]["version"]
            .as_u64()
            .ok_or_else(|| format!("{REPO}: no version for {name}"))
    };
    let timestamp = parse(files, sign::TIMESTAMP_PATH)?;
    let snapshot = parse(
        files,
        &sign::path("snapshot", version(&timestamp, "snapshot.json")?),
    )?;
    parse(
        files,
        &sign::path(role, version(&snapshot, &format!("{role}.json"))?),
    )
}

fn parse(files: &[(String, Vec<u8>)], path: &str) -> Result<Value, String> {
    let (_, bytes) = files
        .iter()
        .find(|(p, _)| p == path)
        .ok_or_else(|| format!("{REPO} has no {path}"))?;
    serde_json::from_slice(bytes).map_err(|e| format!("{REPO}/{path}: {e}"))
}

/// The repository's files, by path from its top.
struct InMemory<'a>(&'a [(String, Vec<u8>)]);

impl Files for InMemory<'_> {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        match self.0.iter().find(|(p, _)| p == path) {
            Some((_, bytes)) if bytes.len() as u64 > max => {
                Err(format!("{path} is over {max} bytes"))
            }
            Some((_, bytes)) => Ok(Some(bytes.clone())),
            None => Ok(None),
        }
    }
}

/// Every `.json` file in `repo`'s `metadata/`, in name order.
fn read_repository(repo: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let dir = repo.join(image::METADATA);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    let file = |name: String| {
        let bytes = fs::read(dir.join(&name)).map_err(|e| format!("cannot read {name}: {e}"))?;
        Ok((format!("{}/{name}", image::METADATA), bytes))
    };
    names.into_iter().map(file).collect()
}

/// `files` as `repo`'s `metadata/`: every other role file there goes, and a
/// committed root must be `files`' byte for byte.
fn write_repository(repo: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let dir = repo.join(image::METADATA);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    for (path, _) in read_repository(repo)? {
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

/// Makes `work`, and removes the `.canon` and `.sig` files an earlier run left.
fn clear(work: &Path) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("cannot make {}: {e}", work.display()))?;
    let entries = fs::read_dir(work).map_err(|e| format!("cannot list {}: {e}", work.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("canon" | "sig")
        ) {
            fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
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
