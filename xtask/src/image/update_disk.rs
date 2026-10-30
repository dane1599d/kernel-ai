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

use crate::pkgrepo::tuf::{self, Key, SPEC_VERSION};
use kaipkg::generation::Time;
use serde_json::json;
use std::sync::OnceLock;

/// The target an update disk carries, as `updated` looks it up.
pub const TARGET: &str = "slot.img";
const DAY: u64 = 86_400;

/// This run's throwaway test root: four keys and the time the metadata counts
/// its expiry from, made once so every boot of a chained case shares them.
pub struct TestRoot {
    root: Key,
    targets: Key,
    snapshot: Key,
    timestamp: Key,
    now: Time,
}

/// The test root, made on first use.
pub fn test_root() -> Result<&'static TestRoot, String> {
    static ROOT: OnceLock<TestRoot> = OnceLock::new();
    if let Some(root) = ROOT.get() {
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
    Ok(ROOT.get_or_init(|| made))
}

impl TestRoot {
    fn expires(&self, days: u64) -> String {
        Time(self.now.0 + days * DAY).format()
    }

    /// `1.root.json`: the device's trust anchor, `kai.root.json` in a slot's bundle.
    pub fn root_json(&self) -> Result<Vec<u8>, String> {
        let root = json!({
            "_type": "root",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": self.expires(365),
            "consistent_snapshot": true,
            "keys": tuf::key_map(&[&self.root, &self.targets, &self.snapshot, &self.timestamp]),
            "roles": {
                "root": tuf::role(&self.root),
                "targets": tuf::role(&self.targets),
                "snapshot": tuf::role(&self.snapshot),
                "timestamp": tuf::role(&self.timestamp),
            },
        });
        tuf::envelope(&root, &self.root)
    }

    /// The update disk carrying `image` as release `release_seq`. With
    /// `tampered`, one byte of the image on the disk is flipped after signing.
    pub fn disk(&self, image: &[u8], release_seq: u32, tampered: bool) -> Result<Vec<u8>, String> {
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
        let snapshot = json!({
            "_type": "snapshot",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": self.expires(30),
            "meta": { "targets.json": { "version": 1 } },
        });
        let snapshot = tuf::envelope(&snapshot, &self.snapshot)?;
        let timestamp = json!({
            "_type": "timestamp",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": self.expires(7),
            "meta": {
                "snapshot.json": {
                    "version": 1,
                    "length": snapshot.len(),
                    "hashes": { "sha256": crate::sha256_hex(&snapshot) },
                },
            },
        });
        let mut body = image.to_vec();
        if tampered {
            let middle = body.len() / 2;
            body[middle] ^= 0xFF;
        }
        let target_path = format!("targets/{TARGET}");
        let files: Vec<(String, Vec<u8>)> = vec![
            ("metadata/1.root.json".into(), self.root_json()?),
            (
                "metadata/1.targets.json".into(),
                tuf::envelope(&targets, &self.targets)?,
            ),
            ("metadata/1.snapshot.json".into(), snapshot),
            (
                "metadata/timestamp.json".into(),
                tuf::envelope(&timestamp, &self.timestamp)?,
            ),
            (target_path, body),
        ];
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
}
