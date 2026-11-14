//! `scripts/release-sign.sh` with OpenSSL 3, run as the runbook runs it
//! (`env -i bash --noprofile --norc`), on key files encrypted as the founder's
//! are (PKCS#8, scrypt). CI's runner has `/usr/bin/openssl` 3, so there the
//! test must run; elsewhere it is skipped when that is missing.

use super::*;
use std::io::Write;
use std::process::{Command, Stdio};

const OPENSSL: &str = "/usr/bin/openssl";
const ROOT_PASS: &str = "root test passphrase";
const RELEASE_PASS: &str = "release test passphrase";
/// RFC 8032 §7.1's test 2: the release key's signature of the one byte 0x72.
const TEST_2_SIG: &str = concat!(
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da",
    "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"
);

fn openssl_3() -> bool {
    let version = Command::new(OPENSSL).arg("version").output();
    let ok = version.is_ok_and(|out| out.stdout.starts_with(b"OpenSSL 3."));
    if !ok && std::env::var_os("CI").is_some() {
        panic!("CI runs the OpenSSL round trip: {OPENSSL} must be OpenSSL 3");
    }
    if !ok {
        eprintln!("skipped: no OpenSSL 3 at {OPENSSL}");
    }
    ok
}

fn run(command: &mut Command, input: &str) -> std::process::Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// `seed` as an encrypted key file at `path`, as the runbook makes one.
fn key_file(path: &Path, seed: &str, pass: &str) {
    let der = path.with_extension("der");
    let pkcs8 = kaituf::unhex(&format!("302e020100300506032b657004220420{seed}")).unwrap();
    fs::write(&der, pkcs8).unwrap();
    let mut pkcs8 = Command::new(OPENSSL);
    pkcs8.args(["pkcs8", "-topk8", "-v2", "aes-256-cbc", "-scrypt"]);
    // The runbook's cost: N as high as OpenSSL's 32 MiB scrypt limit allows
    // (2^20 is refused), and p for the rest.
    pkcs8.args(["-scrypt_N", "16384", "-scrypt_r", "8", "-scrypt_p", "64"]);
    pkcs8.args(["-inform", "DER"]);
    pkcs8
        .arg("-in")
        .arg(&der)
        .arg("-out")
        .arg(path)
        .args(["-passout", "stdin"]);
    let out = run(&mut pkcs8, &format!("{pass}\n"));
    fs::remove_file(&der).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The script, as the runbook runs it, its passphrases on stdin.
fn sign_script(keys: &Path, work: &Path, input: &str) -> std::process::Output {
    let script = crate::workspace_root().join("scripts/release-sign.sh");
    let mut env = Command::new("env");
    env.args(["-i", "/bin/bash", "--noprofile", "--norc"]);
    env.arg(script).arg(keys).arg(work);
    run(&mut env, input)
}

fn stick(paths: &Paths) -> PathBuf {
    let stick = paths.work.with_file_name("stick");
    fs::create_dir_all(&stick).unwrap();
    key_file(&stick.join("root.key"), ROOT_SEED, ROOT_PASS);
    key_file(&stick.join("release.key"), RELEASE_SEED, RELEASE_PASS);
    // The public halves, as setup step 6 leaves them beside the keys.
    fs::write(stick.join("root.pub"), pem(&key(ROOT_SEED))).unwrap();
    fs::write(stick.join("release.pub"), pem(&key(RELEASE_SEED))).unwrap();
    stick
}

/// Before any passphrase, the script names the stick's two keys and each
/// key a file names, in hex: the digits the founder holds on paper.
#[test]
fn the_script_shows_every_key_before_a_passphrase() {
    if !openssl_3() {
        return;
    }
    let paths = workspace("showkeys");
    let stick = stick(&paths);
    prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
    // No passphrase given: it stops at the first prompt, having shown the keys.
    let out = sign_script(&stick, &paths.work, "");
    let shown = String::from_utf8(out.stdout).unwrap();
    let [root, release] =
        [ROOT_SEED, RELEASE_SEED].map(|s| sign::hex_bytes(&key(s).public_key().0));
    assert!(shown.contains(&format!("root.key is {root}")), "{shown}");
    assert!(
        shown.contains(&format!("release.key is {release}")),
        "{shown}"
    );
    let names = |file: &str| {
        let line = shown
            .lines()
            .find(|l| l.starts_with(&format!("{file}.canon names:")))
            .unwrap();
        let mut keys: Vec<&str> = line.split_whitespace().skip(2).collect();
        keys.sort_unstable();
        keys
    };
    let mut both = vec![root.as_str(), release.as_str()];
    both.sort_unstable();
    assert_eq!(names("root"), both);
    assert_eq!(names("targets"), [release.as_str()]);
    assert!(names("snapshot").is_empty());
    // Which round, and of which release.
    let round = "round 1 of release 1: signs models packages root snapshot targets\n";
    assert!(shown.contains(round), "{shown}");
    // Each model and package it signs, as the prepare job lists them, a
    // package with its grants.
    for model in models::catalog().unwrap() {
        let line = format!(
            "models.canon signs {}, sha256 {}, length {}\n",
            model.target(),
            model.sha256,
            model.length
        );
        assert!(shown.contains(&line), "{line} in {shown}");
    }
    for (path, bytes) in packages::built(&targets()) {
        let line = format!(
            "packages.canon signs {path}, sha256 {}, grants []\n",
            crate::sha256_hex(&bytes)
        );
        assert!(shown.contains(&line), "{line} in {shown}");
    }
}

/// A `packages.canon` entry of another shape than xtask writes, a second
/// hash or a field of its own, stops the script before any passphrase; a
/// grant added shows on its line.
#[test]
fn the_script_refuses_a_package_entry_xtask_does_not_write() {
    if !openssl_3() {
        return;
    }
    let edits: [Edit; 3] = [
        ("sha512", PACKAGES, |s| {
            s["targets"]["notes-1.0.0.kpkg"]["hashes"]["sha512"] = json!("00")
        }),
        ("field", PACKAGES, |s| {
            s["targets"]["notes-1.0.0.kpkg"]["mirror"] = json!("x")
        }),
        ("custom", PACKAGES, |s| {
            s["targets"]["notes-1.0.0.kpkg"]["custom"]["extra"] = json!({})
        }),
    ];
    for (name, role, edit) in edits {
        let paths = workspace(&format!("script-{name}"));
        let stick = stick(&paths);
        prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
        tamper(&paths.work, role, edit);
        let out = sign_script(&stick, &paths.work, "");
        let said = String::from_utf8(out.stderr).unwrap();
        assert!(!out.status.success(), "{name}");
        assert!(
            said.contains("packages.canon is not what xtask writes"),
            "{name}: {said}"
        );
        assert!(!said.contains("Passphrase"), "{name}: {said}");
    }
    let paths = workspace("script-grant");
    let stick = stick(&paths);
    prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
    tamper(&paths.work, PACKAGES, |s| {
        s["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["grants"] = json!(["net"])
    });
    let shown = String::from_utf8(sign_script(&stick, &paths.work, "").stdout).unwrap();
    assert!(
        shown.contains(r#"packages.canon signs notes-1.0.0.kpkg, sha256 "#)
            && shown.contains(r#", grants ["net"]"#),
        "{shown}"
    );
}

/// A way to hide an entry from the script's lines: an edit, then raw bytes
/// swapped in the canonical file, and how the script refuses it.
struct Hidden {
    name: &'static str,
    edit: fn(&mut Value),
    from: &'static str,
    to: &'static str,
    refused: &'static str,
}

/// An honest copy of notes' entry in a top-level field that sorts before
/// `targets`, while the real entry is granted `net`.
fn decoy(s: &mut Value) {
    let honest = s["targets"]["notes-1.0.0.kpkg"].clone();
    s["a"] = json!({ "targets": { "notes-1.0.0.kpkg": honest } });
    s["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["grants"] = json!(["net"]);
}

/// A `packages.canon` that would show the founder honest lines while signing
/// another entry stops the script before any passphrase, its lines unprinted:
/// a decoy field before `targets`, a raw line feed in a key or a grant, or a
/// line feed at the end.
#[test]
fn the_script_refuses_a_decoy_or_a_line_feed_in_packages() {
    if !openssl_3() {
        return;
    }
    const PRINTABLE: &str = "packages.canon is not one line of printable ASCII";
    let cases = [
        Hidden {
            name: "decoy",
            edit: decoy,
            from: "",
            to: "",
            refused: "packages.canon is not what xtask writes",
        },
        Hidden {
            name: "decoy-lf",
            edit: decoy,
            from: r#""a":{"targets":{"notes-1.0.0.kpkg""#,
            to: "\"a\":{\"targets\":{\"notes-1.0.0.kp\nkg\"",
            refused: PRINTABLE,
        },
        Hidden {
            name: "grant-lf",
            edit: |s| s["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["grants"] = json!(["net"]),
            from: r#"["net"]"#,
            to: "[\"ne\nt\"]",
            refused: PRINTABLE,
        },
        Hidden {
            name: "end-lf",
            edit: |_| {},
            from: "",
            to: "\n",
            refused: PRINTABLE,
        },
    ];
    for case in cases {
        let name = case.name;
        let paths = workspace(&format!("script-hidden-{name}"));
        let stick = stick(&paths);
        prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
        tamper(&paths.work, PACKAGES, case.edit);
        let path = paths.work.join("packages.canon");
        let mut text = fs::read_to_string(&path).unwrap();
        match case.from {
            "" => text.push_str(case.to),
            from => {
                assert!(text.contains(from), "{name}: {text}");
                text = text.replacen(from, case.to, 1);
            }
        }
        fs::write(&path, text).unwrap();
        let out = sign_script(&stick, &paths.work, "");
        let said = String::from_utf8(out.stderr).unwrap();
        let shown = String::from_utf8(out.stdout).unwrap();
        assert!(!out.status.success(), "{name}");
        assert!(said.contains(case.refused), "{name}: {said}");
        assert!(!said.contains("Passphrase"), "{name}: {said}");
        assert!(!shown.contains("packages.canon signs"), "{name}: {shown}");
    }
}

/// `targets.canon` and `models.canon` are signed with release.key too, and
/// the script prints no line of theirs to check, while the device's search
/// would ask top-level `targets` first. So each must be exactly what xtask
/// writes: a package in targets' own map, the delegations reordered with
/// models first and taking `*.kpkg`, or a package in models' map stops the
/// script before any passphrase.
#[test]
fn the_script_refuses_targets_or_models_that_could_speak_for_a_package() {
    if !openssl_3() {
        return;
    }
    let edits: [Edit; 3] = [
        ("toplevel", "targets", |s| {
            s["targets"]["notes-1.0.0.kpkg"] = sign::target_entry(&targets()[1])
        }),
        ("reordered", "targets", |s| {
            let roles = s["delegations"]["roles"].as_array_mut().unwrap();
            roles.reverse();
            roles[0]["paths"] = json!(["*.kpkg", "*.gguf", "*.chunks"]);
        }),
        ("modelkpkg", MODELS, |s| {
            s["targets"]["notes-1.0.0.kpkg"] = sign::target_entry(&targets()[1])
        }),
    ];
    for (name, role, edit) in edits {
        let paths = workspace(&format!("script-role-{name}"));
        let stick = stick(&paths);
        prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
        tamper(&paths.work, role, edit);
        let out = sign_script(&stick, &paths.work, "");
        let said = String::from_utf8(out.stderr).unwrap();
        let shown = String::from_utf8(out.stdout).unwrap();
        assert!(!out.status.success(), "{name}");
        let refused = format!("{role}.canon is not what xtask writes");
        assert!(said.contains(&refused), "{name}: {said}");
        assert!(!shown.contains("packages.canon signs"), "{name}: {shown}");
        let sigs = fs::read_dir(&paths.work).unwrap().flatten();
        let signed = sigs.filter(|e| e.path().extension().is_some_and(|x| x == "sig"));
        assert_eq!(signed.count(), 0, "{name}");
    }
}

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

/// How many `.sig` files `work` holds.
fn sigs(work: &Path) -> usize {
    let entries = fs::read_dir(work).unwrap().flatten();
    entries
        .filter(|e| e.path().extension().is_some_and(|x| x == "sig"))
        .count()
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
    ];
    for case in cases {
        let name = case.name;
        let paths = workspace(&format!("script-round-{name}"));
        let stick = stick(&paths);
        prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
        if case.second {
            sign_here(&paths.work);
            prepare_timestamp(&paths, NOW).unwrap();
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

/// RFC 8032 test 2 through the script, then a whole release in its two
/// rounds: OpenSSL's raw signatures and `openssl pkey -pubout`'s keys are
/// what `kaipkg::sign` makes, byte for byte.
#[test]
fn the_script_signs_as_kaipkg_does() {
    if !openssl_3() {
        return;
    }
    let paths = workspace("openssl");
    let stick = stick(&paths);
    let vector = paths.work.with_file_name("vector");
    fs::create_dir_all(&vector).unwrap();
    fs::write(vector.join("targets.canon"), [0x72]).unwrap();
    let out = sign_script(&stick, &vector, &format!("{RELEASE_PASS}\n"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(vector.join("targets.sig"))
            .unwrap()
            .trim(),
        TEST_2_SIG
    );

    let mut pubout = Command::new(OPENSSL);
    pubout
        .args(["pkey", "-pubout", "-passin", "stdin", "-in"])
        .arg(stick.join("root.key"));
    let public = run(&mut pubout, &format!("{ROOT_PASS}\n"));
    assert_eq!(
        String::from_utf8(public.stdout).unwrap(),
        pem(&key(ROOT_SEED))
    );

    let models = models::catalog().unwrap();
    let both = format!("{ROOT_PASS}\n{RELEASE_PASS}\n");
    let script = |work: &Path| {
        let input = match work.join("root.canon").exists() && !work.join("root.sig").exists() {
            true => both.clone(),
            false => format!("{RELEASE_PASS}\n"),
        };
        let out = sign_script(&stick, work, &input);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    release(&paths, &models, 1, &script).unwrap();
    assert_eq!(read_repository(&paths.repo).unwrap(), in_one_go(&models));
}

/// A wrong passphrase signs nothing.
#[test]
fn a_wrong_passphrase_signs_nothing() {
    if !openssl_3() {
        return;
    }
    let paths = workspace("wrongpass");
    let stick = stick(&paths);
    prepare(&paths, &targets(), &models::catalog().unwrap(), 1, NOW).unwrap();
    let out = sign_script(
        &stick,
        &paths.work,
        &format!("{RELEASE_PASS}\n{RELEASE_PASS}\n"),
    );
    assert!(!out.status.success());
    let sigs = fs::read_dir(&paths.work).unwrap().flatten();
    assert_eq!(
        sigs.filter(|e| e.path().extension().is_some_and(|x| x == "sig"))
            .count(),
        0
    );
}
