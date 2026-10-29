//! Links every program with `link.ld`, and writes `tufbench`'s fixture.

use std::env;
use std::path::PathBuf;

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    println!("cargo:rustc-link-arg-bins=-T{dir}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    tuf_bench::write(&out.join("tufbench"));
}

/// `tufbench`'s fixture (src/bin/tufbench.rs): root, timestamp, snapshot, targets
/// and a `packages` role of 1,000 targets with their `custom.kai` index fields,
/// each signed by its own ECDSA P-256 key, as YubiKeys and the cloud KMS sign
/// (ADR 0008 §7), and one Ed25519 signature, as a device key makes. Signed here
/// on the host so no signing code reaches the program; ECDSA by RFC 6979, so the
/// bytes are the same on every build.
mod tuf_bench {
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer as _;
    use serde_json::{Map, Value, json};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::Path;

    pub const TARGETS: usize = 1000;
    const EXPIRES: &str = "2099-01-01T00:00:00Z";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    struct Key(SigningKey);

    impl Key {
        fn new(seed: u8) -> Self {
            Key(SigningKey::from_slice(&[seed; 32]).expect("a valid scalar"))
        }

        fn public(&self) -> Value {
            let point = self.0.verifying_key().to_sec1_point(false);
            json!({ "keytype": "ecdsa", "scheme": "ecdsa-sha2-nistp256", "keyval": { "public": hex(point.as_bytes()) } })
        }

        fn id(&self) -> String {
            hex(&Sha256::digest(
                kaituf::canonical::canonical(&self.public()).expect("a key is JSON"),
            ))
        }

        fn envelope(&self, signed: Value) -> Vec<u8> {
            let message = kaituf::canonical::canonical(&signed).expect("metadata is JSON");
            let sig: p256::ecdsa::Signature = self.0.sign(&message);
            let sig = hex(sig.to_der().as_bytes());
            let all =
                json!({ "signed": signed, "signatures": [{ "keyid": self.id(), "sig": sig }] });
            serde_json::to_vec(&all).expect("metadata serializes")
        }
    }

    fn role(key: &Key) -> Value {
        json!({ "keyids": [key.id()], "threshold": 1 })
    }

    pub fn write(dir: &Path) {
        fs::create_dir_all(dir).expect("OUT_DIR is writable");
        let [root, timestamp, snapshot, targets, packages] = [1, 2, 3, 4, 5].map(Key::new);
        let keys: Map<String, Value> = [&root, &timestamp, &snapshot, &targets]
            .iter()
            .map(|k| (k.id(), k.public()))
            .collect();
        let root_signed = json!({
            "_type": "root", "spec_version": "1.0.31", "version": 1, "expires": EXPIRES,
            "consistent_snapshot": true, "keys": keys,
            "roles": { "root": role(&root), "timestamp": role(&timestamp), "snapshot": role(&snapshot), "targets": role(&targets) },
        });
        let mut entries = Map::new();
        for n in 0..TARGETS {
            let name = format!("app-{n:04}");
            let bytes = format!("kai-pkg 1 {name}");
            entries.insert(format!("{name}-1.0.{n}.kpkg"), json!({
                "length": 74_000 + n,
                "hashes": { "sha256": hex(&Sha256::digest(bytes.as_bytes())) },
                "custom": { "kai": {
                    "name": name, "version": format!("1.0.{n}"), "kind": "app",
                    "summary": "An app the AI can open, read and write for you",
                    "licence": "MPL-2.0", "world": "kai:app@0.1", "depends": ["kai:app ^0.1"],
                    "grants": [], "actions": [format!("{name}_open"), format!("{name}_read"), format!("{name}_write")],
                    "yanked": false,
                } },
            }));
        }
        let packages_signed = json!({
            "_type": "targets", "spec_version": "1.0.31", "version": 1, "expires": EXPIRES, "targets": entries,
        });
        let mut delegated = Map::new();
        delegated.insert(packages.id(), packages.public());
        let targets_signed = json!({
            "_type": "targets", "spec_version": "1.0.31", "version": 1, "expires": EXPIRES, "targets": {},
            "delegations": {
                "keys": delegated,
                "roles": [{ "name": "packages", "keyids": [packages.id()], "threshold": 1, "terminating": true, "paths": ["*.kpkg"] }],
            },
        });
        let snapshot_signed = json!({
            "_type": "snapshot", "spec_version": "1.0.31", "version": 1, "expires": EXPIRES,
            "meta": { "targets.json": { "version": 1 }, "packages.json": { "version": 1 } },
        });
        let snapshot_bytes = snapshot.envelope(snapshot_signed);
        let timestamp_signed = json!({
            "_type": "timestamp", "spec_version": "1.0.31", "version": 1, "expires": EXPIRES,
            "meta": { "snapshot.json": {
                "version": 1, "length": snapshot_bytes.len(), "hashes": { "sha256": hex(&Sha256::digest(&snapshot_bytes)) },
            } },
        });
        let root_bytes = root.envelope(root_signed);
        // A device key's Ed25519 signature of root's canonical bytes.
        let pair = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([6; 32]));
        let message: Value = serde_json::from_slice(&root_bytes).expect("root is JSON");
        let message = kaituf::canonical::canonical(&message["signed"]).expect("root is canonical");
        let device = json!({ "public": hex(pair.pk.as_ref()), "sig": hex(pair.sk.sign(&message, None).as_ref()) });
        for (name, bytes) in [
            ("1.root.json", root_bytes),
            ("timestamp.json", timestamp.envelope(timestamp_signed)),
            ("1.snapshot.json", snapshot_bytes),
            ("1.targets.json", targets.envelope(targets_signed)),
            ("1.packages.json", packages.envelope(packages_signed)),
            ("ed25519.json", serde_json::to_vec(&device).expect("JSON")),
        ] {
            fs::write(dir.join(name), bytes).expect("OUT_DIR is writable");
        }
    }
}
