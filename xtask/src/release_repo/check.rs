//! What `assemble` checks before it writes, and `release-image` before it
//! bundles: the repository verifies as a device checks it, and every byte the
//! founder signed is one `prepare` writes for these keys and this catalog. A
//! session that edited a `.canon` file before it was signed, as with a
//! delegation to a key of its own or an expiry in 2999, is caught here.

use super::{FileKeys, KEYS, REPO, plan};
use crate::pkgrepo::models::Model;
use kaipkg::generation::Time;
use kaipkg::image;
use kaipkg::sign::{self, ADR_0008, MODELS, PACKAGES};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// A repository's files, by path from its top.
pub type RepoFiles = Vec<(String, Vec<u8>)>;

/// The version the non-root roles may have.
#[derive(Clone, Copy)]
pub(super) enum Version {
    /// `assemble`'s: the `release_seq` that `prepare` signs at.
    Exactly(u64),
    /// A committed repository's: one release's, not above `release_seq`.
    AtMost(u64),
}

/// The checks, on `files` and at `now`:
/// - they are the six files a release writes, the non-root roles at one
///   version that `version` allows;
/// - they verify as a device checks the image's (`kaipkg::image::listed`), with
///   no package and no model refused, and root carries its own key's signature;
/// - each role's `signed` is what `prepare` writes for `models` and `keys` at
///   that version, but its expiry, which is no later than `now` plus its period.
pub(super) fn check_repository(
    files: &[(String, Vec<u8>)],
    models: &[Model],
    keys: &FileKeys,
    now: Time,
    version: Version,
) -> Result<(), String> {
    let timestamp = parse(files, sign::TIMESTAMP_PATH)?;
    let at = timestamp["signed"]["version"].as_u64().unwrap_or(0);
    let allowed = match version {
        Version::Exactly(want) => at == want,
        Version::AtMost(most) => (1..=most).contains(&at),
    };
    if !allowed {
        return Err(format!("{REPO}: version {at} is not this release's"));
    }
    let mut names: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    names.sort_unstable();
    let roles = ["targets", PACKAGES, MODELS, "snapshot"].map(|r| sign::path(r, at));
    let mut want: Vec<&str> = roles.iter().map(String::as_str).collect();
    want.extend([image::ROOT, sign::TIMESTAMP_PATH]);
    want.sort_unstable();
    if names != want {
        return Err(format!(
            "{REPO} holds {names:?}, not the files a release writes"
        ));
    }
    let mut repository = InMemory(files);
    let listed = image::listed(&mut repository, MODELS).map_err(|why| format!("{REPO}: {why}"))?;
    if let Some((path, why)) = listed.refused.first() {
        return Err(format!("{REPO}: {path} is refused: {why}"));
    }
    if !image::listed(&mut repository, PACKAGES)?.targets.is_empty() {
        return Err(format!("{REPO}: a release's packages role lists nothing"));
    }
    root_signed(files, keys)?;
    let plan = plan(keys, models, at, Time(0))?;
    let snapshot = bytes(files, &sign::path("snapshot", at))?;
    let stamp = sign::timestamp(snapshot, at, Time(0), ADR_0008);
    let models_role = plan.models.as_ref().ok_or("the plan has no models role")?;
    let p = ADR_0008;
    let expected = [
        (image::ROOT, &plan.root, p.root),
        (roles[0].as_str(), &plan.targets, p.targets),
        (roles[1].as_str(), &plan.packages, p.targets),
        (roles[2].as_str(), models_role, p.targets),
        (roles[3].as_str(), &plan.snapshot, p.snapshot),
        (sign::TIMESTAMP_PATH, &stamp, p.timestamp),
    ];
    for (path, want, days) in expected {
        let got = parse(files, path)?;
        if !same_but_expiry(&got["signed"], want) {
            return Err(format!(
                "{REPO}/{path} is not what `release-repo prepare` writes for {KEYS} and \
                 xtask/models.json: sign it again"
            ));
        }
        expires_within(&got["signed"], now, days).map_err(|why| format!("{REPO}/{path}: {why}"))?;
    }
    Ok(())
}

/// Whether two roles' `signed` objects are equal but for `expires`.
pub(super) fn same_but_expiry(got: &Value, want: &Value) -> bool {
    let without = |value: &Value| {
        let mut value = value.clone();
        if let Some(fields) = value.as_object_mut() {
            fields.remove("expires");
        }
        value
    };
    without(got) == without(want)
}

/// `signed` expires no later than `days` after `now`.
fn expires_within(signed: &Value, now: Time, days: u64) -> Result<(), String> {
    let bound = Time(now.0 + days * sign::DAY).format();
    match signed["expires"].as_str() {
        Some(at) if at.len() == bound.len() && at <= bound.as_str() => Ok(()),
        _ => Err(format!("it expires later than {days} days from now")),
    }
}

/// Root carries a signature by the root key over its own `signed`: the image
/// repository takes the root it carries as trusted, so this is its check.
pub(super) fn root_signed(files: &[(String, Vec<u8>)], keys: &FileKeys) -> Result<(), String> {
    let root = parse(files, image::ROOT)?;
    let canon = sign::canonical(&root["signed"])?;
    let key = kaituf::keys::Key::parse(&keys.root.public()).map_err(|e| e.to_string())?;
    let id = keys.root.id();
    let by_root = |sig: &Value| {
        sig["keyid"] == id.as_str() && sig["sig"].as_str().is_some_and(|s| key.verifies(&canon, s))
    };
    match root["signatures"]
        .as_array()
        .is_some_and(|sigs| sigs.iter().any(by_root))
    {
        true => Ok(()),
        false => Err(format!(
            "{REPO}/{}: no valid signature by root.key",
            image::ROOT
        )),
    }
}

/// The committed repository, checked as [`check_repository`] does at `now`,
/// its version not above `release_seq`, or none when nothing is committed.
pub fn committed(root: &Path) -> Result<Option<RepoFiles>, String> {
    let repo = root.join(REPO);
    if !repo.join(image::ROOT).exists() {
        return Ok(None);
    }
    let keys = FileKeys::read(&root.join(KEYS))?;
    let files = read_repository(&repo)?;
    let most = crate::release_image::release_seq(root)?.into();
    let now = crate::pkgrepo::unix_now()?;
    let models = crate::pkgrepo::models::catalog()?;
    check_repository(&files, &models, &keys, now, Version::AtMost(most))?;
    Ok(Some(files))
}

fn bytes<'a>(files: &'a [(String, Vec<u8>)], path: &str) -> Result<&'a [u8], String> {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, bytes)| bytes.as_slice())
        .ok_or_else(|| format!("{REPO} has no {path}"))
}

fn parse(files: &[(String, Vec<u8>)], path: &str) -> Result<Value, String> {
    serde_json::from_slice(bytes(files, path)?).map_err(|e| format!("{REPO}/{path}: {e}"))
}

/// The repository's files, by path from its top.
struct InMemory<'a>(&'a [(String, Vec<u8>)]);

impl image::Files for InMemory<'_> {
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
pub(super) fn read_repository(repo: &Path) -> Result<RepoFiles, String> {
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
