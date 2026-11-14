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
    stick
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
    prepare(&paths, &models::catalog().unwrap(), 1, NOW).unwrap();
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
