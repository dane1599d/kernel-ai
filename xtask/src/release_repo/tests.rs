//! `release-repo`'s steps with keys signed in the test (ed25519-compact), and
//! once through `scripts/release-sign.sh` and OpenSSL 3 where they are.

use super::*;
use kaipkg::sign::{Key, Keys};
use serde_json::json;

/// RFC 8032 §7.1's test 1 seed, as the root key.
const ROOT_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
/// RFC 8032 §7.1's test 2 seed, as the release key.
const RELEASE_SEED: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
const NOW: Time = Time(1_800_000_000);

fn key(seed: &str) -> Key {
    Key::from_seed(kaipkg::unhex(seed).unwrap())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| n << 8 | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            match i <= chunk.len() {
                true => out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }
    out
}

/// `key`'s public half as `openssl pkey -pubout` writes it.
fn pem(key: &Key) -> String {
    let der = [&ED25519_SPKI[..], &key.public_key().0].concat();
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        base64(&der)
    )
}

/// A fresh workspace with the two public keys committed.
fn workspace(name: &str) -> Paths {
    let dir = std::env::temp_dir().join(format!("kai-release-repo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let paths = Paths::of(&dir, dir.join("work"));
    fs::create_dir_all(&paths.keys).unwrap();
    fs::write(paths.keys.join("root.pub"), pem(&key(ROOT_SEED))).unwrap();
    fs::write(paths.keys.join("release.pub"), pem(&key(RELEASE_SEED))).unwrap();
    paths
}

/// Signs each `.canon` with no `.sig` as the script does, in the test.
fn sign_here(work: &Path) {
    for entry in fs::read_dir(work).unwrap().flatten() {
        let path = entry.path();
        let sig = path.with_extension("sig");
        if path.extension().is_none_or(|e| e != "canon") || sig.exists() {
            continue;
        }
        let seed = match path.file_stem().unwrap() == "root" {
            true => ROOT_SEED,
            false => RELEASE_SEED,
        };
        let raw = key(seed).sign_bytes(&fs::read(&path).unwrap());
        fs::write(sig, format!("{}\n", sign::hex_bytes(&raw))).unwrap();
    }
}

/// Two built-in apps as packages, as `packages::pack` lists them: the bytes
/// stand in for a packed `.kpkg`, which needs the components built.
fn targets() -> Vec<Target> {
    let package = |name: &str, bytes: &[u8]| Target {
        path: format!("{name}-1.0.0.kpkg"),
        bytes: bytes.to_vec(),
        kai: json!({
            "name": name,
            "version": "1.0.0",
            "grants": [],
            "actions": [format!("{name}_find")],
            "yanked": false,
        }),
    };
    vec![
        package("files", b"files package"),
        package("notes", b"notes package"),
    ]
}

/// `packages::targets`' stand-in: each package `prepare` wrote is one of
/// [`targets`], whose index it gets back, as a real one packed again from its
/// artifact makes its own.
fn repack(built: &[(String, Vec<u8>)]) -> Result<Vec<Target>, String> {
    let known = |(path, bytes): &(String, Vec<u8>)| {
        targets()
            .into_iter()
            .find(|t| t.path == *path && t.bytes == *bytes)
            .ok_or_else(|| format!("{path} packs again to other bytes"))
    };
    built.iter().map(known).collect()
}

fn release(
    paths: &Paths,
    models: &[Model],
    version: u64,
    sign: &dyn Fn(&Path),
) -> Result<(), String> {
    release_at(paths, models, version, sign, NOW)
}

/// [`release`] at `now`: the sign script holds expiries to its own clock.
fn release_at(
    paths: &Paths,
    models: &[Model],
    version: u64,
    sign: &dyn Fn(&Path),
    now: Time,
) -> Result<(), String> {
    prepare(paths, &targets(), models, version, now)?;
    sign(&paths.work);
    prepare_timestamp(paths, now)?;
    sign(&paths.work);
    assemble(paths, models, version, now, &repack)
}

/// What `kaipkg::sign` writes with both keys in hand: the metadata files.
fn in_one_go(models: &[Model]) -> Vec<(String, Vec<u8>)> {
    in_one_go_at(models, NOW)
}

/// [`in_one_go`] at `now`.
fn in_one_go_at(models: &[Model], now: Time) -> Vec<(String, Vec<u8>)> {
    let (root, release) = (key(ROOT_SEED), key(RELEASE_SEED));
    let keys = Keys {
        root: &root,
        models: Some(&release),
        ..Keys::one(&release)
    };
    let models = tuf::sign_models(models).unwrap();
    let mut files: Vec<_> = sign::build_with_models(&targets(), &models, &keys, now, ADR_0008)
        .unwrap()
        .into_iter()
        .filter(|(path, _)| path.starts_with("metadata/"))
        .collect();
    files.sort();
    files
}

/// Round 1's folder as `prepare` leaves it for the test keys (RFC 8032's test 1
/// seed as root, test 2's as release) at this machine's clock, written to
/// `$KAI_SIGN_FIXTURE`. `release.yml`'s macOS job signs it with the sign
/// script in both rounds, under bash 3.2 and BSD awk, as the founder's Mac
/// would.
#[test]
#[ignore = "release.yml's sign-fixture job runs it"]
fn write_round_one_for_the_macos_job() {
    let out =
        std::env::var_os("KAI_SIGN_FIXTURE").expect("KAI_SIGN_FIXTURE names the folder to write");
    let out = PathBuf::from(out);
    let _ = fs::remove_dir_all(&out);
    let paths = Paths::of(&out, out.join("work"));
    fs::create_dir_all(&paths.keys).unwrap();
    fs::write(paths.keys.join("root.pub"), pem(&key(ROOT_SEED))).unwrap();
    fs::write(paths.keys.join("release.pub"), pem(&key(RELEASE_SEED))).unwrap();
    let now = crate::pkgrepo::unix_now().unwrap();
    prepare(&paths, &targets(), &models::catalog().unwrap(), 1, now).unwrap();
}

#[test]
fn reads_a_public_key_as_openssl_writes_it() {
    let rfc = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=\n-----END PUBLIC KEY-----\n";
    assert_eq!(read_public(rfc), Ok(key(ROOT_SEED).public_key()));
    assert_eq!(pem(&key(ROOT_SEED)), rfc);
    assert!(read_public("MCowBQYDK2VwAyEA11qY").is_err());
    let p256 = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64(&[0x30; 91])
    );
    assert_eq!(read_public(&p256), Err("not an Ed25519 public key".into()));
}

/// The steps apart, with the signatures made elsewhere, write what signing in
/// one go does, byte for byte (Ed25519 signs deterministically), and that
/// repository checks out as the committed one must.
#[test]
fn signed_apart_the_release_is_what_signing_in_one_go_writes() {
    let paths = workspace("apart");
    let models = models::catalog().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let written = read_repository(&paths.repo).unwrap();
    assert_eq!(written, in_one_go(&models));
    let keys = FileKeys::read(&paths.keys).unwrap();
    // The packages `prepare` wrote are the ones it signed.
    assert_eq!(
        packages::read(&paths.work).unwrap(),
        packages::built(&targets())
    );
    let built = targets();
    let sums = fs::read_to_string(paths.work.join("packages/SHA256SUMS")).unwrap();
    assert!(sums.contains("  notes-1.0.0.kpkg\n"), "{sums}");
    check_repository(
        &written,
        &models,
        Some(&built),
        &keys,
        NOW,
        Mode::Signed { version: 1 },
    )
    .unwrap();
}

/// A second release reuses the committed root byte for byte and names its
/// other roles by the new version; the old ones go.
#[test]
fn a_later_release_keeps_the_frozen_root() {
    let paths = workspace("frozen");
    let models = models::catalog().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let root = fs::read(paths.repo.join(image::ROOT)).unwrap();
    let later = Time(NOW.0 + 86_400);
    prepare(&paths, &targets(), &models, 2, later).unwrap();
    assert!(!paths.work.join("root.canon").exists());
    sign_here(&paths.work);
    prepare_timestamp(&paths, later).unwrap();
    sign_here(&paths.work);
    assemble(&paths, &models, 2, later, &repack).unwrap();
    let names: Vec<String> = read_repository(&paths.repo)
        .unwrap()
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let want = [
        "1.root",
        "2.models",
        "2.packages",
        "2.snapshot",
        "2.targets",
        "timestamp",
    ];
    assert_eq!(names, want.map(|n| format!("metadata/{n}.json")));
    assert_eq!(fs::read(paths.repo.join(image::ROOT)).unwrap(), root);
    // A root that names other keys is never re-signed to fit them.
    fs::write(paths.keys.join("release.pub"), pem(&key(ROOT_SEED))).unwrap();
    let refused = prepare(&paths, &targets(), &models, 3, NOW).unwrap_err();
    assert!(refused.contains("frozen"), "{refused}");
}

#[test]
fn a_signature_that_is_not_the_keys_is_refused_by_name() {
    let paths = workspace("badsig");
    let models = models::catalog().unwrap();
    prepare(&paths, &targets(), &models, 1, NOW).unwrap();
    sign_here(&paths.work);
    prepare_timestamp(&paths, NOW).unwrap();
    sign_here(&paths.work);
    let sig = paths.work.join("models.sig");
    let text = fs::read_to_string(&sig).unwrap();
    let flipped = if text.starts_with('0') { "1" } else { "0" };
    fs::write(&sig, format!("{flipped}{}", &text[1..])).unwrap();
    let refused = assemble(&paths, &models, 1, NOW, &repack).unwrap_err();
    assert_eq!(
        refused,
        "models.sig is not release.key's signature of models.canon"
    );
    assert!(!paths.repo.exists(), "nothing is written");
}

/// Canonical JSON writes control characters and UTF-8 as themselves, and the
/// sign script signs only one line of printable ASCII: `prepare` writes no
/// other, so the founder is never handed a file the script refuses.
#[test]
fn a_canon_outside_printable_ascii_is_not_written() {
    let models = models::catalog().unwrap();
    for (name, summary) in [("lf", "Notes\nand more"), ("utf8", "Notes \u{2014} more")] {
        let paths = workspace(&format!("ascii-{name}"));
        let mut odd = targets();
        odd[1].kai["summary"] = json!(summary);
        let refused = prepare(&paths, &odd, &models, 1, NOW).unwrap_err();
        assert!(
            refused.starts_with("packages.canon has byte 0x"),
            "{name}: {refused}"
        );
        assert!(refused.ends_with("release-sign.sh refuses it"), "{refused}");
    }
}

#[test]
fn a_catalog_model_without_a_chunk_list_is_not_signed() {
    let paths = workspace("nolist");
    let mut models = models::catalog().unwrap();
    models[0].chunks = None;
    let refused = prepare(&paths, &targets(), &models, 1, NOW).unwrap_err();
    assert!(refused.contains("no committed chunk list"), "{refused}");
}

/// A committed role that no longer lists the catalog, as after a model is
/// added to `xtask/models.json`, fails the check `release-image` makes.
#[test]
fn a_models_role_that_is_not_the_catalog_fails_the_check() {
    let paths = workspace("drift");
    let mut models = models::catalog().unwrap();
    let last = models.pop().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let keys = FileKeys::read(&paths.keys).unwrap();
    let files = read_repository(&paths.repo).unwrap();
    check_repository(
        &files,
        &models,
        None,
        &keys,
        NOW,
        Mode::Committed { most: 1 },
    )
    .unwrap();
    // Within 60 days of root's expiry, the committed repository no longer
    // passes: root version 2 is due.
    let late = Time(NOW.0 + 320 * sign::DAY);
    let refused = check_repository(
        &files,
        &models,
        None,
        &keys,
        late,
        Mode::Committed { most: 1 },
    );
    assert!(
        refused
            .unwrap_err()
            .contains("1.root.json: it expires sooner than 60 days")
    );
    models.push(last);
    let refused = check_repository(
        &files,
        &models,
        None,
        &keys,
        NOW,
        Mode::Committed { most: 1 },
    )
    .unwrap_err();
    assert!(
        refused.contains("1.models.json is not what `release-repo prepare` writes"),
        "{refused}"
    );
}

/// A build whose packages are not the signed ones is refused, naming the
/// package and both hashes, as is a package signed and not built or built and
/// not signed. With no build (`ci.yml`), the role is held by its shape alone.
#[test]
fn packages_not_the_signed_ones_are_named_with_both_hashes() {
    let paths = workspace("packages");
    let models = models::catalog().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let keys = FileKeys::read(&paths.keys).unwrap();
    let files = read_repository(&paths.repo).unwrap();
    let check = |built: Option<&[Target]>| {
        check_repository(
            &files,
            &models,
            built,
            &keys,
            NOW,
            Mode::Committed { most: 1 },
        )
    };
    check(None).unwrap();
    let mut built = targets();
    check(Some(&built)).unwrap();
    built[1].bytes = b"notes, rebuilt".to_vec();
    let refused = check(Some(&built)).unwrap_err();
    let (signed, packed) = (
        crate::sha256_hex(b"notes package"),
        crate::sha256_hex(b"notes, rebuilt"),
    );
    assert!(refused.starts_with("notes-1.0.0.kpkg: "), "{refused}");
    assert!(
        refused.contains(&signed) && refused.contains(&packed),
        "{refused}"
    );
    let refused = check(Some(&built[..1])).unwrap_err();
    assert!(
        refused.contains("signs notes-1.0.0.kpkg, which this build does not pack"),
        "{refused}"
    );
    let mut more = targets();
    more.push(Target {
        path: "terminal-1.0.0.kpkg".into(),
        bytes: b"terminal".to_vec(),
        kai: json!({ "name": "terminal", "version": "1.0.0" }),
    });
    let refused = check(Some(&more)).unwrap_err();
    assert!(
        refused.contains("packs terminal-1.0.0.kpkg, which"),
        "{refused}"
    );
}

/// A package's index changed in `packages.canon` before it was signed, a
/// grant added or an action renamed, is refused by name though its hash is
/// the build's: the OS acts on the index. An entry of another shape, a second
/// hash or a field of its own, is refused with no build too.
#[test]
fn a_package_index_changed_before_it_was_signed_is_refused() {
    let models = models::catalog().unwrap();
    let edits: [(Edit, &str); 3] = [
        (
            ("grant", PACKAGES, |s| {
                s["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["grants"] = json!(["net"])
            }),
            r#"grants ["net"] where this build has []"#,
        ),
        (
            ("action", PACKAGES, |s| {
                s["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["actions"] = json!(["notes_wipe"])
            }),
            r#"actions ["notes_wipe"] where this build has ["notes_find"]"#,
        ),
        (
            ("sha512", PACKAGES, |s| {
                s["targets"]["notes-1.0.0.kpkg"]["hashes"]["sha512"] = json!("00")
            }),
            "notes-1.0.0.kpkg has hashes other than one SHA-256",
        ),
    ];
    for ((name, role, edit), want) in edits {
        let paths = workspace(name);
        prepare(&paths, &targets(), &models, 1, NOW).unwrap();
        tamper(&paths.work, role, edit);
        sign_here(&paths.work);
        prepare_timestamp(&paths, NOW).unwrap();
        sign_here(&paths.work);
        let refused = assemble(&paths, &models, 1, NOW, &repack).unwrap_err();
        assert!(refused.contains(want), "{name}: {refused}");
        assert!(!paths.repo.exists(), "{name}: nothing is written");
    }
    let mut listed = json!({ "notes-1.0.0.kpkg": sign::target_entry(&targets()[1]) });
    packages::check(&listed, None, REPO).unwrap();
    let entry = &mut listed["notes-1.0.0.kpkg"];
    entry["custom"]["kai"]["yanked"] = json!(true);
    let refused = packages::check(&listed, None, REPO).unwrap_err();
    assert!(refused.ends_with("notes-1.0.0.kpkg is yanked"), "{refused}");
    listed["notes-1.0.0.kpkg"]["mirror"] = json!("elsewhere");
    let refused = packages::check(&listed, None, REPO).unwrap_err();
    assert!(refused.contains("fields other than custom"), "{refused}");
}

/// A later root committed beside version 1 is never deleted to make room.
#[test]
fn a_later_root_file_stops_assemble() {
    let paths = workspace("laterroot");
    let models = models::catalog().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let later = paths.repo.join("metadata/2.root.json");
    fs::write(&later, b"{}").unwrap();
    let refused = release(&paths, &models, 1, &sign_here).unwrap_err();
    assert!(refused.contains("2.root.json is committed"), "{refused}");
    assert!(later.exists());
}

/// A name for the case, the role edited, and the edit.
type Edit = (&'static str, &'static str, fn(&mut Value));

/// `<role>.canon` in `work` with `edit` applied, as a session that reaches the
/// work folder could rewrite it before it is signed.
fn tamper(work: &Path, role: &str, edit: fn(&mut Value)) {
    let path = work.join(format!("{role}.canon"));
    let mut signed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    edit(&mut signed);
    fs::write(&path, sign::canonical(&signed).unwrap()).unwrap();
}

/// Bytes the founder signed that `prepare` would not write are refused even
/// with good signatures: a delegation widened, an expiry far off, or a root
/// that would expire soon after it is frozen.
#[test]
fn a_role_edited_before_it_was_signed_is_refused() {
    let models = models::catalog().unwrap();
    let edits: [Edit; 3] = [
        ("widened", "targets", |s| {
            s["delegations"]["roles"][1]["paths"] = json!(["*"])
        }),
        ("far-off", "snapshot", |s| {
            s["expires"] = json!("2999-01-01T00:00:00Z")
        }),
        ("short-lived", "root", |s| {
            s["expires"] = json!("2027-01-20T00:00:00Z")
        }),
    ];
    for (name, role, edit) in edits {
        let paths = workspace(name);
        prepare(&paths, &targets(), &models, 1, NOW).unwrap();
        tamper(&paths.work, role, edit);
        sign_here(&paths.work);
        prepare_timestamp(&paths, NOW).unwrap();
        sign_here(&paths.work);
        let refused = assemble(&paths, &models, 1, NOW, &repack).unwrap_err();
        assert!(
            refused.contains(&format!("1.{role}.json")),
            "{name}: {refused}"
        );
        assert!(!paths.repo.exists(), "{name}: nothing is written");
    }
}

/// Roles signed at another version than this release's are refused.
#[test]
fn a_version_other_than_this_releases_is_refused() {
    let paths = workspace("version");
    let models = models::catalog().unwrap();
    prepare(&paths, &targets(), &models, 1, NOW).unwrap();
    sign_here(&paths.work);
    prepare_timestamp(&paths, NOW).unwrap();
    sign_here(&paths.work);
    let refused = assemble(&paths, &models, 2, NOW, &repack).unwrap_err();
    assert!(
        refused.contains("version 1 is not this release's"),
        "{refused}"
    );
}

/// A committed root whose signature is not root.key's is never reused.
#[test]
fn a_committed_root_not_signed_by_root_key_is_refused() {
    let paths = workspace("rootsig");
    let models = models::catalog().unwrap();
    release(&paths, &models, 1, &sign_here).unwrap();
    let at = paths.repo.join(image::ROOT);
    let mut root: Value = serde_json::from_slice(&fs::read(&at).unwrap()).unwrap();
    root["signatures"][0]["sig"] = json!("00".repeat(64));
    fs::write(&at, sign::canonical(&root).unwrap()).unwrap();
    let refused = prepare(&paths, &targets(), &models, 2, NOW).unwrap_err();
    assert!(
        refused.contains("no valid signature by root.key"),
        "{refused}"
    );
}

/// The runbook gives the script's SHA-256, which the founder checks against
/// the copy on the key stick; the script stays short and offline.
#[test]
fn the_runbook_pins_the_sign_script() {
    let root = crate::workspace_root();
    let script = fs::read(root.join("scripts/release-sign.sh")).unwrap();
    assert!(!script.contains(&b'\r'), "release-sign.sh is LF only");
    let sha = crate::sha256_hex(&script);
    let doc = fs::read_to_string(root.join("docs/release/root-ceremony.md")).unwrap();
    assert!(
        doc.contains(&sha),
        "root-ceremony.md must give release-sign.sh's SHA-256 {sha}"
    );
    let text = String::from_utf8(script).unwrap();
    assert!(
        text.lines().count() <= 300,
        "release-sign.sh stays short enough to read"
    );
    for word in ["curl", "wget", "/dev/tcp", "ssh", "nc ", "git ", "http"] {
        assert!(
            !text.contains(word),
            "release-sign.sh makes no network call: {word}"
        );
    }
}

#[cfg(unix)]
mod openssl;
