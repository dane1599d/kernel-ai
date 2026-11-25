//! The script signs only a round's own files, as xtask writes them: each
//! folder that holds something else is refused before any passphrase.

use super::*;

/// A folder that holds something other than one round's own files, as xtask
/// writes them, and how the script refuses it.
struct Round {
    name: &'static str,
    /// The second round's folder: round 1 signed, then `prepare-timestamp`.
    second: bool,
    change: fn(&Path),
    refused: &'static str,
}

/// `<role>.canon` in `work` with its first `from` replaced by `to`.
fn swap(work: &Path, role: &str, from: &str, to: &str) {
    let path = work.join(format!("{role}.canon"));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{role}.canon has no {from}");
    fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

/// The first key `root.canon` names, in hex.
fn first_key(work: &Path) -> String {
    let text = fs::read_to_string(work.join("root.canon")).unwrap();
    let (_, rest) = text.split_once("\"public\":\"").unwrap();
    rest[..64].to_string()
}

/// `<role>.canon` in `work` set to expire `days` from now.
fn redate(work: &Path, role: &str, days: u64) {
    let path = work.join(format!("{role}.canon"));
    let text = fs::read_to_string(&path).unwrap();
    let (head, rest) = text.split_once(r#""expires":""#).unwrap();
    let at = Time(clock().0 + days * sign::DAY).format();
    fs::write(&path, format!(r#"{head}"expires":"{at}{}"#, &rest[20..])).unwrap();
}

/// release.key signs every role but root, and the device tells roles apart
/// by `_type` and by the snapshot's versions alone. So the script signs only a
/// round's own files, each the text xtask writes at one release number, with
/// plain keys: a targets role as `timestamp.canon`, beside round 1's files or
/// as round 2's, a snapshot of another `_type`, a round missing a file, an
/// upper-case or PEM key, a role at another version, and a model id the OS
/// refuses each stop it before any passphrase, and nothing is signed.
#[test]
fn the_script_signs_only_a_rounds_own_files_as_xtask_writes_them() {
    if !openssl_3() {
        return;
    }
    const KEY: &str = "root.canon names a key that is not lower-case hex Ed25519";
    const NOT_A_ROUND: &str = "not one round's files";
    let timestamp_is_packages = |w: &Path| {
        fs::copy(w.join("packages.canon"), w.join("timestamp.canon")).unwrap();
    };
    let cases = [
        Round {
            name: "extra",
            second: false,
            change: timestamp_is_packages,
            refused: NOT_A_ROUND,
        },
        Round {
            name: "typed",
            second: true,
            change: timestamp_is_packages,
            refused: "timestamp.canon is not what xtask writes",
        },
        Round {
            name: "snapshot-type",
            second: false,
            change: |w| {
                swap(
                    w,
                    "snapshot",
                    r#""_type":"snapshot""#,
                    r#""_type":"targets""#,
                )
            },
            refused: "is not a snapshot",
        },
        Round {
            name: "missing",
            second: false,
            change: |w| fs::remove_file(w.join("models.canon")).unwrap(),
            refused: NOT_A_ROUND,
        },
        Round {
            name: "upper-key",
            second: false,
            change: |w| {
                let key = first_key(w);
                swap(w, "root", &key, &key.to_uppercase())
            },
            refused: KEY,
        },
        Round {
            name: "pem-key",
            second: false,
            change: |w| {
                let pem = "-----BEGIN PUBLIC KEY-----MCowBQYDK2VwAyEA-----END PUBLIC KEY-----";
                swap(w, "root", &first_key(w), pem)
            },
            refused: KEY,
        },
        Round {
            name: "version",
            second: false,
            change: |w| swap(w, "packages", r#","version":1}"#, r#","version":2}"#),
            refused: "packages.canon is not what xtask writes",
        },
        Round {
            name: "model-id",
            second: false,
            change: |w| swap(w, "models", r#""qwen3-0.6b.gguf""#, r#""Qwen3-0.6b.gguf""#),
            refused: "models.canon is not what xtask writes",
        },
        Round {
            name: "root-v2",
            second: false,
            change: |w| swap(w, "root", r#","version":1}"#, r#","version":2}"#),
            refused: "root.canon is not what xtask writes",
        },
        Round {
            name: "root-roles",
            second: false,
            change: |w| {
                let text = fs::read_to_string(w.join("root.canon")).unwrap();
                let root: Value = serde_json::from_str(&text).unwrap();
                let ids = &root["roles"];
                let (root_id, release_id) = (
                    ids["root"]["keyids"][0].as_str().unwrap(),
                    ids["targets"]["keyids"][0].as_str().unwrap(),
                );
                let targets = format!(r#""targets":{{"keyids":["{release_id}"]"#);
                swap(w, "root", &targets, &targets.replace(release_id, root_id))
            },
            refused: "root.canon is not what xtask writes",
        },
        Round {
            name: "root-keyid",
            second: false,
            change: |w| {
                let text = fs::read_to_string(w.join("root.canon")).unwrap();
                let root: Value = serde_json::from_str(&text).unwrap();
                let id = root["roles"]["targets"]["keyids"][0]
                    .as_str()
                    .unwrap()
                    .to_string();
                let fake = "f".repeat(64);
                fs::write(w.join("root.canon"), text.replace(&id, &fake)).unwrap();
            },
            refused: "root.canon is not what xtask writes",
        },
        Round {
            name: "far",
            second: false,
            change: |w| redate(w, "packages", 730),
            refused: "packages.canon expires",
        },
        Round {
            name: "near",
            second: false,
            change: |w| redate(w, "snapshot", 20),
            refused: "not within a week of 30",
        },
        Round {
            name: "timestamp-hash",
            second: true,
            change: |w| {
                let text = fs::read_to_string(w.join("timestamp.canon")).unwrap();
                let (_, rest) = text.split_once(r#""sha256":""#).unwrap();
                swap(w, "timestamp", &rest[..64], &"0".repeat(64))
            },
            refused: "timestamp.canon is not what xtask writes",
        },
        Round {
            name: "snapshot-sig",
            second: true,
            change: |w| {
                let sig = fs::read_to_string(w.join("snapshot.sig")).unwrap();
                let flipped = match sig.starts_with('0') {
                    true => format!("1{}", &sig[1..]),
                    false => format!("0{}", &sig[1..]),
                };
                fs::write(w.join("snapshot.sig"), flipped).unwrap();
            },
            refused: "snapshot.sig is not release.key's signature of snapshot.canon",
        },
        Round {
            name: "spec",
            second: false,
            change: |w| {
                swap(
                    w,
                    "snapshot",
                    r#""spec_version":"1.0.37""#,
                    r#""spec_version":"1.0.38""#,
                )
            },
            refused: "snapshot.canon is not what xtask writes",
        },
        Round {
            name: "not-a-date",
            second: false,
            change: |w| {
                let text = fs::read_to_string(w.join("packages.canon")).unwrap();
                let (_, rest) = text.split_once(r#""expires":""#).unwrap();
                swap(w, "packages", &rest[..20], "2027-02-30T00:00:00Z")
            },
            refused: "packages.canon expires 2027-02-30T00:00:00Z, not a real date",
        },
        Round {
            name: "signed-before",
            second: false,
            change: |w| ledger(w, &round_one(1, &"0".repeat(64))),
            refused: "release 1 is not above 1, the last these keys signed",
        },
        Round {
            name: "longer-number",
            second: false,
            change: |w| ledger(w, &round_one(10, &"0".repeat(64))),
            refused: "release 1 is not above 10, the last these keys signed",
        },
        Round {
            name: "no-ledger",
            second: false,
            change: |w| fs::remove_file(w.with_file_name("stick").join("signed.log")).unwrap(),
            refused: "has no signed.log, made at setup",
        },
        Round {
            name: "torn",
            second: false,
            change: |w| ledger(w, "1 1 models=00"),
            refused: "signed.log's last line is torn",
        },
        Round {
            name: "not-a-ledger-line",
            second: false,
            change: |w| ledger(w, "1 1 models=0\n"),
            refused: "signed.log has a line this script does not write",
        },
        Round {
            name: "no-round-one",
            second: true,
            change: |w| ledger(w, ""),
            refused: "is not the last round these keys signed",
        },
    ];
    for case in cases {
        let name = case.name;
        let paths = workspace(&format!("script-round-{name}"));
        let stick = stick(&paths);
        prepare(&paths, &targets(), &models::catalog().unwrap(), 1, clock()).unwrap();
        if case.second {
            sign_here(&paths.work);
            prepare_timestamp(&paths, clock()).unwrap();
            // What the script's round 1 would have logged on the stick.
            let snapshot = crate::sha256_hex(&fs::read(paths.work.join("snapshot.canon")).unwrap());
            ledger(&paths.work, &round_one(1, &snapshot));
        }
        (case.change)(&paths.work);
        let before = sigs(&paths.work);
        let out = sign_script(&stick, &paths.work, "");
        let said = String::from_utf8(out.stderr).unwrap();
        assert!(!out.status.success(), "{name}");
        assert!(said.contains(case.refused), "{name}: {said}");
        assert_eq!(sigs(&paths.work), before, "{name}: nothing is signed");
    }
}
