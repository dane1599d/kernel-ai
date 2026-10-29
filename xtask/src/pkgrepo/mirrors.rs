//! The mirrors the `pkg-install` cases boot against (P4 task 2): the image's
//! packages and [`big`], signed by the image's own per-build keys, so the root
//! the image carries is the one each is checked against, and ADR 0011's five
//! ways to tamper with one.

use super::tuf::{self, Keys, envelope};
use super::{Package, Repository};
use kaipkg::bundle;
use kaipkg::generation::Time;
use kaipkg::header::Header;
use serde_json::{Value, json};

/// Which mirror a case boots against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MirrorKind {
    /// As published.
    Signed,
    /// One byte of `big`'s object flipped.
    FlippedByte,
    /// Signed a week and a day ago: its timestamp has expired.
    Expired,
    /// Snapshot and timestamp at version 2, for the first boot of the rollback case.
    Newer,
    /// A timestamp at version 3 naming snapshot 1, older than [`Self::Newer`]'s.
    RolledBack,
    /// Signed by keys the image's root does not name.
    UnknownKey,
    /// `big`'s index says 1.0.0; the bytes it serves say 9.9.9.
    HeaderDiffers,
}

/// The package the cases install: a 300 KiB `data` package, two of the store's
/// 192 KiB pieces.
pub fn big() -> Package {
    package("big", "1.0.0", &vec![7u8; 300 << 10])
}

/// A `data` package named `name` at `version` holding `payload`.
pub fn package(name: &str, version: &str, payload: &[u8]) -> Package {
    let header_text = format!(
        "kai-package 1\nname {name}\nversion {version}\nkind data\nsummary A test package\nlicence MIT\n"
    );
    Package {
        header: Header::parse(&header_text).expect("a test package's header parses"),
        bytes: bundle::build(&[(bundle::HEADER, header_text.as_bytes()), ("data", payload)]),
        header_text,
        actions: Vec::new(),
        grants: Vec::new(),
    }
}

/// `repo` with `path`'s bytes changed by `edit`.
pub fn tampered(repo: &Repository, path: &str, edit: impl Fn(&mut Vec<u8>)) -> Repository {
    let files = repo
        .files
        .iter()
        .map(|(p, bytes)| {
            let mut bytes = bytes.clone();
            if p == path {
                edit(&mut bytes);
            }
            (p.clone(), bytes)
        })
        .collect();
    Repository { files }
}

fn file<'a>(repo: &'a Repository, path: &str) -> Result<&'a [u8], String> {
    let found = repo.files.iter().find(|(p, _)| p == path);
    found
        .map(|(_, bytes)| bytes.as_slice())
        .ok_or_else(|| format!("the repository has no {path}"))
}

/// `repo` with its timestamp at `version` naming snapshot `snapshot_version`,
/// both signed again by `keys`.
pub fn at_version(
    repo: &Repository,
    keys: &Keys,
    version: u64,
    snapshot_version: u64,
) -> Result<Repository, String> {
    let json = |path| -> Result<Value, String> {
        serde_json::from_slice(file(repo, path)?).map_err(|e| format!("{path}: {e}"))
    };
    let mut snapshot = json("metadata/1.snapshot.json")?;
    snapshot["signed"]["version"] = snapshot_version.into();
    let snapshot = envelope(&snapshot["signed"], &keys.snapshot)?;
    let mut timestamp = json("metadata/timestamp.json")?;
    let signed = &mut timestamp["signed"];
    signed["version"] = version.into();
    signed["meta"]["snapshot.json"] = json!({
        "version": snapshot_version,
        "length": snapshot.len(),
        "hashes": { "sha256": crate::sha256_hex(&snapshot) },
    });
    let timestamp = envelope(signed, &keys.timestamp)?;
    let mut files: Vec<(String, Vec<u8>)> = repo
        .files
        .iter()
        .filter(|(p, _)| p != "metadata/timestamp.json")
        .cloned()
        .collect();
    files.push((
        format!("metadata/{snapshot_version}.snapshot.json"),
        snapshot,
    ));
    files.push(("metadata/timestamp.json".into(), timestamp));
    Ok(Repository { files })
}

/// The `kind` mirror of `packages` (the image's) and [`big`], signed by `keys`
/// at `now`.
pub fn build(
    kind: MirrorKind,
    packages: &[Package],
    keys: &Keys,
    now: Time,
) -> Result<Repository, String> {
    let mut all = packages.to_vec();
    let big = big();
    let target = format!("targets/{}.big-1.0.0.kpkg", crate::sha256_hex(&big.bytes));
    match kind {
        MirrorKind::HeaderDiffers => {
            let mut liar = big;
            liar.bytes = package("big", "9.9.9", &vec![7u8; 300 << 10]).bytes;
            all.push(liar);
        }
        _ => all.push(big),
    }
    let signed = |at: Time| tuf::build(&all, keys, at);
    Ok(match kind {
        MirrorKind::Signed | MirrorKind::HeaderDiffers => signed(now)?,
        MirrorKind::FlippedByte => tampered(&signed(now)?, &target, |b| b[200_000] ^= 1),
        MirrorKind::Expired => signed(Time(now.0.saturating_sub(8 * 86_400)))?,
        MirrorKind::Newer => at_version(&signed(now)?, keys, 2, 2)?,
        MirrorKind::RolledBack => at_version(&signed(now)?, keys, 3, 1)?,
        MirrorKind::UnknownKey => tuf::build(&all, &Keys::generate()?, now)?,
    })
}
