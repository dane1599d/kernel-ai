//! The update disk (ADR 0008 §12): before P5's downloads, an update arrives as
//! a disk with QEMU serial `kai-update`, holding a copy of the TUF repository as
//! one `kai-bundle 1`:
//!
//! ```text
//! metadata/1.root.json
//! metadata/1.targets.json
//! metadata/1.snapshot.json
//! metadata/timestamp.json
//! targets/slot.img
//! ```
//!
//! The target sits under its own path, not consistent snapshots' `<sha256>.`
//! name, which is past a bundle path's 64 bytes; its SHA-256 is checked anyway.
//!
//! `slot.img` is a slot image (`slots::slot_image`) whose target names its
//! `release_seq` in `custom.kai`. It is signed by the test root of this xtask
//! run (P4 plan section 4, item 3), the same root every slot case's bundle
//! carries as `kai.root.json`, so `updated` checks it as it will a download.
//!
//! The key-rotation drill (P8 row 3, with throwaway roots only): [`rotated`]
//! is a second test root, v2, with four keys of its own. An update disk
//! signed under it ([`Signer::Root2`]) carries `metadata/2.root.json`, root
//! version 2 naming v2's keys and signed by v1's root key and v2's. A device
//! that trusts v1 moves to v2 on reading it, and from then refuses metadata
//! signed under v1 alone.

use crate::pkgrepo::tuf::{self, Key, SPEC_VERSION};
use kaipkg::generation::Time;
use kaipkg::sign;
use serde_json::json;
use std::sync::OnceLock;

/// The target an update disk carries, as `updated` looks it up.
pub const TARGET: &str = "slot.img";
/// An online release's chunk list, a sibling target (`updatenet::plan`).
pub const CHUNKS_TARGET: &str = "slot.img.chunks";
const DAY: u64 = 86_400;

/// The versions and the lifetime of an online repository's timestamp and
/// snapshot: what makes a repository newer, older or lapsed than the one a
/// device trusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// The timestamp's version.
    pub timestamp: u64,
    /// The snapshot's version, which the timestamp names.
    pub snapshot: u64,
    /// Days from now until the timestamp expires; negative is expired.
    pub days: i64,
}

impl Default for Stamp {
    /// Version 1 of both, valid for a week.
    fn default() -> Self {
        Self {
            timestamp: 1,
            snapshot: 1,
            days: 7,
        }
    }
}

impl Stamp {
    /// The timestamp lapsed a day ago.
    pub fn expired() -> Self {
        Self {
            days: -1,
            ..Self::default()
        }
    }

    /// Version 2 of both: newer than [`Stamp::default`], which a device that
    /// trusted this one then refuses to go back from.
    pub fn newer() -> Self {
        Self {
            timestamp: 2,
            snapshot: 2,
            ..Self::default()
        }
    }

    /// A timestamp at version 3 naming snapshot 1: older than [`Stamp::newer`]'s.
    pub fn rolled_back() -> Self {
        Self {
            timestamp: 3,
            snapshot: 1,
            ..Self::default()
        }
    }
}

/// Which test root signs an update disk's metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signer {
    /// The run's test root, v1: what every slot trusts as `kai.root.json`.
    Root1,
    /// The rotated root, v2, with `2.root.json` signed by v1's root key and
    /// v2's.
    Root2,
    /// [`Self::Root2`], but `2.root.json` signed by v2's root key alone: a
    /// rotation the device must refuse, since v1's root role never signed it.
    Root2OneKey,
}

/// This run's throwaway test root: four keys and the time the metadata counts
/// its expiry from, made once so every boot of a chained case shares them.
pub struct TestRoot {
    root: Key,
    targets: Key,
    snapshot: Key,
    timestamp: Key,
    now: Time,
}

fn make(cell: &'static OnceLock<TestRoot>) -> Result<&'static TestRoot, String> {
    if let Some(root) = cell.get() {
        return Ok(root);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("the clock is before 1970: {e}"))?;
    let made = TestRoot {
        root: tuf::generate_key()?,
        targets: tuf::generate_key()?,
        snapshot: tuf::generate_key()?,
        timestamp: tuf::generate_key()?,
        now: Time(now.as_secs()),
    };
    Ok(cell.get_or_init(|| made))
}

/// The test root, v1, made on first use.
pub fn test_root() -> Result<&'static TestRoot, String> {
    static ROOT: OnceLock<TestRoot> = OnceLock::new();
    make(&ROOT)
}

/// The rotated test root, v2: four keys none of v1's, made on first use.
/// Only [`TestRoot::rotation`] names them, as root version 2.
pub fn rotated() -> Result<&'static TestRoot, String> {
    static ROOT: OnceLock<TestRoot> = OnceLock::new();
    make(&ROOT)
}

/// `signed` with a signature by each of `keys`, as canonical JSON.
fn envelope_by(signed: &serde_json::Value, keys: &[&Key]) -> Result<Vec<u8>, String> {
    let bytes = sign::canonical(signed)?;
    let signatures: Vec<_> = keys
        .iter()
        .map(|k| json!({ "keyid": k.id(), "sig": sign::hex_bytes(&k.sign_bytes(&bytes)) }))
        .collect();
    sign::canonical(&json!({ "signed": signed, "signatures": signatures }))
}

impl TestRoot {
    fn expires(&self, days: i64) -> String {
        Time(self.now.0.saturating_add_signed(days * DAY as i64)).format()
    }

    /// `1.root.json`: the device's trust anchor, `kai.root.json` in a slot's bundle.
    pub fn root_json(&self) -> Result<Vec<u8>, String> {
        tuf::envelope(&self.root_signed(1), &self.root)
    }

    /// `2.root.json`: root version 2 naming `next`'s keys, signed by this
    /// root's key and `next`'s, as a rotation must be (TUF 5.3.4).
    pub fn rotation(&self, next: &TestRoot) -> Result<Vec<u8>, String> {
        envelope_by(&next.root_signed(2), &[&self.root, &next.root])
    }

    /// The `signed` part of a root at `version` naming this root's four keys.
    fn root_signed(&self, version: u64) -> serde_json::Value {
        let [root, targets, snapshot, timestamp] =
            [&self.root, &self.targets, &self.snapshot, &self.timestamp].map(|k| k.public_key());
        json!({
            "_type": "root",
            "spec_version": SPEC_VERSION,
            "version": version,
            "expires": self.expires(365),
            "consistent_snapshot": true,
            "keys": tuf::key_map(&[&root, &targets, &snapshot, &timestamp]),
            "roles": {
                "root": tuf::role(&root),
                "targets": tuf::role(&targets),
                "snapshot": tuf::role(&snapshot),
                "timestamp": tuf::role(&timestamp),
            },
        })
    }

    /// The four metadata files of a repository whose `targets` role is `targets`
    /// (version 1): `metadata/1.root.json`, `1.targets.json`, `1.snapshot.json`
    /// and `timestamp.json`, signed by this root.
    fn metadata(&self, targets: &serde_json::Value) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.metadata_with(targets, Stamp::default())
    }

    /// [`Self::metadata`] as `stamp` says: the versions of the timestamp and the
    /// snapshot, and when the timestamp expires.
    fn metadata_with(
        &self,
        targets: &serde_json::Value,
        stamp: Stamp,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        let snapshot = json!({
            "_type": "snapshot",
            "spec_version": SPEC_VERSION,
            "version": stamp.snapshot,
            "expires": self.expires(30),
            "meta": { "targets.json": { "version": 1 } },
        });
        let snapshot = tuf::envelope(&snapshot, &self.snapshot)?;
        let timestamp = json!({
            "_type": "timestamp",
            "spec_version": SPEC_VERSION,
            "version": stamp.timestamp,
            "expires": self.expires(stamp.days),
            "meta": {
                "snapshot.json": {
                    "version": stamp.snapshot,
                    "length": snapshot.len(),
                    "hashes": { "sha256": crate::sha256_hex(&snapshot) },
                },
            },
        });
        Ok(vec![
            ("metadata/1.root.json".into(), self.root_json()?),
            (
                "metadata/1.targets.json".into(),
                tuf::envelope(targets, &self.targets)?,
            ),
            (
                format!("metadata/{}.snapshot.json", stamp.snapshot),
                snapshot,
            ),
            (
                "metadata/timestamp.json".into(),
                tuf::envelope(&timestamp, &self.timestamp)?,
            ),
        ])
    }

    /// The seconds since 1970 the metadata counts its expiry from, and a moment
    /// inside its validity: what a device's clock reads in a test.
    #[cfg(test)]
    pub fn clock(&self) -> u64 {
        self.now.0 + 60
    }

    /// An online repository's metadata for release `release_seq`: `slot.img`, a
    /// release asset of GitHub Release `tag`, and its sibling `slot.img.chunks`
    /// holding `list`. The files are `(path, bytes)`: the four metadata files, as
    /// the Pages host serves them. The targets are served as release assets
    /// named `<sha256>.<name>` (consistent snapshots); a test builds those from
    /// the bytes it signs.
    pub fn repository(
        &self,
        image: &[u8],
        list: &[u8],
        release_seq: u32,
        tag: &str,
        stamp: Stamp,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        let targets = json!({
            "_type": "targets",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": self.expires(365),
            "targets": {
                TARGET: {
                    "length": image.len(),
                    "hashes": { "sha256": crate::sha256_hex(image) },
                    "custom": { "kai": { "release_seq": release_seq, "tag": tag } },
                },
                CHUNKS_TARGET: {
                    "length": list.len(),
                    "hashes": { "sha256": crate::sha256_hex(list) },
                },
            },
        });
        self.metadata_with(&targets, stamp)
    }

    /// The update disk carrying `image` as release `release_seq`, signed by
    /// this root. With `tampered`, one byte of the image on the disk is flipped
    /// after signing.
    pub fn disk(&self, image: &[u8], release_seq: u32, tampered: bool) -> Result<Vec<u8>, String> {
        self.disk_from(image, release_seq, tampered, Vec::new())
    }

    /// The update disk signed by `signer`. Under [`Signer::Root2`], the
    /// targets, snapshot and timestamp are v2's, and the disk carries v1's
    /// `1.root.json` and the rotation `2.root.json`.
    pub fn signed_disk(
        image: &[u8],
        release_seq: u32,
        tampered: bool,
        signer: Signer,
    ) -> Result<Vec<u8>, String> {
        let v1 = test_root()?;
        match signer {
            Signer::Root1 => v1.disk(image, release_seq, tampered),
            Signer::Root2 | Signer::Root2OneKey => {
                let v2 = rotated()?;
                let rotation = match signer {
                    Signer::Root2 => v1.rotation(v2)?,
                    _ => envelope_by(&v2.root_signed(2), &[&v2.root])?,
                };
                let roots = vec![
                    ("metadata/1.root.json".into(), v1.root_json()?),
                    ("metadata/2.root.json".into(), rotation),
                ];
                v2.disk_from(image, release_seq, tampered, roots)
            }
        }
    }

    /// The disk of [`Self::disk`], its root files replaced by `roots` when
    /// there are any.
    fn disk_from(
        &self,
        image: &[u8],
        release_seq: u32,
        tampered: bool,
        roots: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<u8>, String> {
        let sha = crate::sha256_hex(image);
        let targets = json!({
            "_type": "targets",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": self.expires(365),
            "targets": {
                TARGET: {
                    "length": image.len(),
                    "hashes": { "sha256": sha },
                    "custom": { "kai": { "release_seq": release_seq } },
                },
            },
        });
        let mut body = image.to_vec();
        if tampered {
            let middle = body.len() / 2;
            body[middle] ^= 0xFF;
        }
        let mut files = self.metadata(&targets)?;
        if !roots.is_empty() {
            files.retain(|(path, _)| !path.ends_with(".root.json"));
            files.extend(roots);
        }
        files.push((format!("targets/{TARGET}"), body));
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let mut disk = kaibundle::build(&refs).map_err(|e| format!("update disk: {e:?}"))?;
        disk.resize(disk.len().next_multiple_of(512), 0);
        Ok(disk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_is_a_bundle_whose_metadata_kaituf_takes_and_whose_target_matches() {
        let keys = test_root().unwrap();
        let image = vec![0x5Au8; 8192];
        let disk = keys.disk(&image, 2, false).unwrap();
        let bundle = kaibundle::parse(&disk).unwrap();
        let root = bundle.get("metadata/1.root.json").unwrap();
        assert_eq!(root, keys.root_json().unwrap());
        let trusted = kaituf::trusted::TrustedSet::new(root, keys.now.0 + 60).unwrap();
        assert!(trusted.root().signed.consistent_snapshot);
        let target = format!("targets/{TARGET}");
        assert_eq!(bundle.get(&target).unwrap(), image);
        let tampered = keys.disk(&image, 2, true).unwrap();
        let bundle = kaibundle::parse(&tampered).unwrap();
        assert_ne!(bundle.get(&target).unwrap(), image);
    }

    /// An update disk as `updated`'s `Remote`, and a cache in memory.
    struct Disk<'a>(kaibundle::Bundle<'a>);
    #[derive(Default)]
    struct Memory(std::collections::BTreeMap<String, Vec<u8>>);

    impl kaituf::updater::Remote for Disk<'_> {
        fn fetch(&mut self, name: &str, _: u64) -> Result<Option<Vec<u8>>, kaituf::Error> {
            Ok(self.0.get(&format!("metadata/{name}")).map(<[u8]>::to_vec))
        }
    }

    impl kaituf::updater::Cache for Memory {
        fn load(&mut self, role: &str) -> Result<Option<Vec<u8>>, kaituf::Error> {
            Ok(self.0.get(role).cloned())
        }
        fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), kaituf::Error> {
            self.0.insert(role.into(), bytes.to_vec());
            Ok(())
        }
    }

    /// Refreshes from `cache` over `disk`, as `updated` does: the root
    /// version it ends on and the release `slot.img` names, or why not.
    fn refresh(disk: &[u8], cache: &mut Memory) -> Result<(u64, u64), String> {
        let mut remote = Disk(kaibundle::parse(disk).map_err(|e| format!("{e:?}"))?);
        let now = test_root().unwrap().clock();
        let mut updater =
            kaituf::updater::Updater::new(&mut remote, cache, now).map_err(|e| e.to_string())?;
        updater.refresh().map_err(|e| e.to_string())?;
        let version = updater.trusted().root().version;
        let target = updater
            .top_level_target(TARGET)
            .map_err(|e| e.to_string())?
            .ok_or("no slot.img")?;
        Ok((
            version,
            target.custom["kai"]["release_seq"].as_u64().unwrap(),
        ))
    }

    /// The drill: a device trusting v1 moves to v2 on a disk signed under v2
    /// and takes its release; from then a disk signed under v1 alone is
    /// refused, and so is a rotation only one of the two root keys signed.
    #[test]
    fn a_device_on_v1_moves_to_v2_and_then_refuses_v1() {
        let (v1, v2) = (test_root().unwrap(), rotated().unwrap());
        let image = vec![0x5Au8; 8192];
        let mut cache = Memory::default();
        cache.0.insert("root".into(), v1.root_json().unwrap());
        let disk = TestRoot::signed_disk(&image, 2, false, Signer::Root2).unwrap();
        assert_eq!(refresh(&disk, &mut cache), Ok((2, 2)));
        assert_eq!(cache.0["root"], v1.rotation(v2).unwrap());

        let old = TestRoot::signed_disk(&image, 3, false, Signer::Root1).unwrap();
        let refused = refresh(&old, &mut cache).unwrap_err();
        assert!(
            refused.contains("below the signature threshold"),
            "{refused}"
        );
        assert_eq!(
            cache.0["root"],
            v1.rotation(v2).unwrap(),
            "the root fell back"
        );

        // The boot case's disk: a rotation signed by v2's root key alone.
        let one = TestRoot::signed_disk(&image, 2, false, Signer::Root2OneKey).unwrap();
        let mut fresh = Memory::default();
        fresh.0.insert("root".into(), v1.root_json().unwrap());
        let got = refresh(&one, &mut fresh).unwrap_err();
        assert!(got.contains("threshold: root has 0 valid"), "{got}");

        // A rotation signed by v2's root key alone, or v1's alone.
        for keys in [[&v2.root], [&v1.root]] {
            let mut bundle: Vec<(String, Vec<u8>)> = kaibundle::parse(&disk)
                .unwrap()
                .entries()
                .map(|e| (e.path.to_string(), e.data.to_vec()))
                .collect();
            let half = envelope_by(&v2.root_signed(2), &keys).unwrap();
            for (path, bytes) in &mut bundle {
                if path == "metadata/2.root.json" {
                    *bytes = half.clone();
                }
            }
            let refs: Vec<(&str, &[u8])> = bundle
                .iter()
                .map(|(p, b)| (p.as_str(), b.as_slice()))
                .collect();
            let half_disk = kaibundle::build(&refs).unwrap();
            let mut fresh = Memory::default();
            fresh.0.insert("root".into(), v1.root_json().unwrap());
            let got = refresh(&half_disk, &mut fresh).unwrap_err();
            assert!(got.contains("below the signature threshold"), "{got}");
            assert_eq!(fresh.0["root"], v1.root_json().unwrap());
        }
    }
}
