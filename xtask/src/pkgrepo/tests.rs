use super::tuf::{self, Key, canonical};
use super::*;
use ed25519_compact::{PublicKey, Signature};
use kaipkg::header::Kind;
use kaipkg::version::Version;
use serde_json::Map;

const NOW: Time = Time(1_793_000_000);

impl Repository {
    fn get(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

/// notes' artifact around a stub module: the real manifest and action rows.
fn notes_artifact() -> Vec<u8> {
    let root = crate::workspace_root();
    let module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let source = artifact::bundle(&[("components/notes/src/lib.rs", b"//")]);
    components::pack(
        &root,
        &components::NOTES_RECIPE,
        APPS[0].label,
        module,
        &source,
    )
    .unwrap()
}

fn notes() -> Package {
    pack(&crate::workspace_root(), &APPS[0], &notes_artifact()).unwrap()
}

fn keys(base: u8) -> Keys {
    let key = |n: u8| Key::from_seed([base.wrapping_add(n); 32]);
    Keys {
        root: key(0),
        targets: key(1),
        snapshot: key(2),
        timestamp: key(3),
        packages: key(4),
    }
}

#[test]
fn notes_packs_with_a_header_that_matches_its_artifact() {
    let artifact = notes_artifact();
    let package = pack(&crate::workspace_root(), &APPS[0], &artifact).unwrap();
    let h = &package.header;
    assert_eq!(h.name, "notes");
    assert_eq!(h.version, Version::new(1, 0, 0));
    assert_eq!(h.kind, Kind::App);
    assert_eq!(h.licence, "MPL-2.0");
    assert_eq!(h.world.as_deref(), Some("kai:app@0.1"));
    assert_eq!(h.depends.len(), 1);
    assert_eq!(h.depends[0].name, "kai:app");
    assert!(h.depends[0].range.matches(&Version::new(0, 1, 9)));
    assert!(!h.depends[0].range.matches(&Version::new(0, 2, 0)));
    let files = bundle::files(&package.bytes).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0], (bundle::HEADER, package.header_text.as_bytes()));
    assert_eq!(files[1], (ARTIFACT, artifact.as_slice()));
    assert!(package.actions.contains(&"notes_add".to_string()));
    assert!(package.actions.iter().all(|a| a.starts_with("notes_")));
    assert_eq!(package.target(), "notes-1.0.0.kpkg");
}

#[test]
fn an_app_whose_parts_disagree_is_refused() {
    let root = crate::workspace_root();
    let artifact = notes_artifact();
    let app = |name, summary, licence| App {
        name,
        label: "2",
        summary,
        licence,
        wit: components::NOTES_RECIPE.wit,
    };
    let refused = |app: App, bytes: &[u8], why: &str| {
        let err = pack(&root, &app, bytes).err().expect("refused");
        assert!(err.contains(why), "{err}");
    };
    refused(
        app("memo", "Notes", "MPL-2.0"),
        &artifact,
        "is component notes",
    );
    refused(
        app("notes", "Notes", "MIT"),
        &artifact,
        "licence is MPL-2.0, not MIT",
    );
    let long = "x".repeat(81).leak();
    refused(app("notes", long, "MPL-2.0"), &artifact, "over 80 bytes");
    refused(
        app("notes", "Notes\u{7}", "MPL-2.0"),
        &artifact,
        "control character",
    );
    refused(
        app("notes", "Notes", "MPL-2.0"),
        b"\0asm\x01\0\0\0",
        "no kai.manifest",
    );
}

#[test]
fn a_missing_boot_version_is_named() {
    let err = pack_all(&crate::workspace_root(), &[]).err().unwrap();
    assert_eq!(err, "comp/notes/2.wasm was not built");
}

/// What a TUF client checks (TUF 1.0.37 §5), over `repo` at `now`: each role
/// signed by its keys in the trusted root, versions and hashes as the
/// timestamp and snapshot name them, unexpired, and each package's bytes and
/// header as its target names them. Returns the packages' bytes by target.
fn verify(repo: &Repository, now: Time) -> Result<Vec<(String, Vec<u8>)>, String> {
    let load = |path: &str| -> Result<(Value, &[u8]), String> {
        let bytes = repo.get(path).ok_or(format!("no {path}"))?;
        let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
        if canonical(&value)? != bytes {
            return Err(format!("{path} is not canonical"));
        }
        Ok((value, bytes))
    };
    let (root, _) = load("metadata/1.root.json")?;
    let root_keys = root["signed"]["keys"]
        .as_object()
        .ok_or("root has no keys")?;
    let trusted = |role: &str| &root["signed"]["roles"][role];
    let root_signed = check(&root, root_keys, trusted("root"), ("root", "root"), now)?;
    assert_eq!(root_signed["consistent_snapshot"], true);

    let (timestamp, _) = load("metadata/timestamp.json")?;
    let ts = check(
        &timestamp,
        root_keys,
        trusted("timestamp"),
        ("timestamp", "timestamp"),
        now,
    )?;
    let (snapshot, snapshot_bytes) = load("metadata/1.snapshot.json")?;
    let named = &ts["meta"]["snapshot.json"];
    if named["length"] != snapshot_bytes.len()
        || named["hashes"]["sha256"] != crate::sha256_hex(snapshot_bytes)
    {
        return Err("the snapshot is not the one the timestamp names".into());
    }
    let snap = check(
        &snapshot,
        root_keys,
        trusted("snapshot"),
        ("snapshot", "snapshot"),
        now,
    )?;
    if snap["version"] != named["version"] {
        return Err("the snapshot's version is not the timestamp's".into());
    }

    let (targets, _) = load("metadata/1.targets.json")?;
    let top = check(
        &targets,
        root_keys,
        trusted("targets"),
        ("targets", "targets"),
        now,
    )?;
    if snap["meta"]["targets.json"]["version"] != top["version"] {
        return Err("targets is not the snapshot's version".into());
    }
    let delegation = &top["delegations"]["roles"][0];
    if delegation["name"] != tuf::PACKAGES || delegation["paths"] != json!(["*.kpkg"]) {
        return Err("targets does not delegate *.kpkg to packages".into());
    }
    let delegated_keys = top["delegations"]["keys"]
        .as_object()
        .ok_or("no delegated keys")?;
    let (packages, _) = load("metadata/1.packages.json")?;
    let pkgs = check(
        &packages,
        delegated_keys,
        delegation,
        ("packages", "targets"),
        now,
    )?;
    if snap["meta"]["packages.json"]["version"] != pkgs["version"] {
        return Err("packages is not the snapshot's version".into());
    }

    let mut out = Vec::new();
    for (target, entry) in pkgs["targets"].as_object().ok_or("no targets")? {
        let sha = entry["hashes"]["sha256"]
            .as_str()
            .ok_or("a target has no sha256")?;
        let bytes = repo
            .get(&format!("targets/{sha}.{target}"))
            .ok_or(format!("no file for {target}"))?;
        if entry["length"] != bytes.len() || crate::sha256_hex(bytes) != sha {
            return Err(format!("{target}'s bytes are not the ones it names"));
        }
        let files = bundle::files(bytes).map_err(String::from)?;
        let header = Header::parse(core::str::from_utf8(files[0].1).map_err(|e| e.to_string())?)?;
        let index = &entry["custom"]["kai"];
        let same = index["name"] == header.name.as_str()
            && index["version"] == header.version.to_string()
            && index["kind"] == header.kind.name()
            && index["summary"] == header.summary.as_str()
            && index["licence"] == header.licence.as_str()
            && index["world"].as_str() == header.world.as_deref()
            && index["depends"].as_array().map(Vec::len) == Some(header.depends.len());
        if !same || *target != format!("{}-{}.kpkg", header.name, header.version) {
            return Err(format!("{target}'s header differs from its index entry"));
        }
        out.push((target.clone(), bytes.to_vec()));
    }
    Ok(out)
}

/// `meta`'s `signed`, if `role`'s threshold of its keys signed it, its type is
/// `kind` and it has not expired at `now`. Errors name the role `name`.
fn check<'a>(
    meta: &'a Value,
    keys: &Map<String, Value>,
    role: &Value,
    (name, kind): (&str, &str),
    now: Time,
) -> Result<&'a Value, String> {
    let signed = &meta["signed"];
    let message = canonical(signed)?;
    let allowed: Vec<&str> = role["keyids"]
        .as_array()
        .ok_or("a role has no keyids")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut good = BTreeSet::new();
    for sig in meta["signatures"].as_array().ok_or("no signatures")? {
        let id = sig["keyid"].as_str().ok_or("a signature has no keyid")?;
        let Some(key) = keys.get(id).filter(|_| allowed.contains(&id)) else {
            continue;
        };
        let public = key["keyval"]["public"]
            .as_str()
            .and_then(unhex)
            .ok_or("a bad public key")?;
        let raw = sig["sig"]
            .as_str()
            .and_then(unhex)
            .ok_or("a bad signature")?;
        let (Ok(public), Ok(raw)) = (PublicKey::from_slice(&public), Signature::from_slice(&raw))
        else {
            continue;
        };
        if public.verify(&message, &raw).is_ok() {
            good.insert(id);
        }
    }
    if (good.len() as u64) < role["threshold"].as_u64().ok_or("no threshold")? {
        return Err(format!("{name}: below its threshold of good signatures"));
    }
    if signed["_type"] != kind || signed["spec_version"] != tuf::SPEC_VERSION {
        return Err(format!("{name}: the wrong type or spec version"));
    }
    let expires = Time::parse(signed["expires"].as_str().ok_or("no expires")?)?;
    if expires <= now {
        return Err(format!("{name}: expired"));
    }
    Ok(signed)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (text.len().is_multiple_of(2))
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect()
        })
        .flatten()
}

#[test]
fn the_repository_verifies_as_a_tuf_client_would() {
    let package = notes();
    let repo = tuf::build(std::slice::from_ref(&package), &keys(1), NOW).unwrap();
    let got = verify(&repo, NOW).unwrap();
    assert_eq!(
        got,
        vec![("notes-1.0.0.kpkg".to_string(), package.bytes.clone())]
    );
    let packages: Value =
        serde_json::from_slice(repo.get("metadata/1.packages.json").unwrap()).unwrap();
    assert_eq!(
        packages["signed"]["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"],
        package.index()
    );
    let mut paths: Vec<&str> = repo.files.iter().map(|(p, _)| p.as_str()).collect();
    paths.sort();
    let target = format!(
        "targets/{}.notes-1.0.0.kpkg",
        crate::sha256_hex(&package.bytes)
    );
    assert_eq!(
        paths,
        [
            "metadata/1.packages.json",
            "metadata/1.root.json",
            "metadata/1.snapshot.json",
            "metadata/1.targets.json",
            "metadata/timestamp.json",
            &target,
        ]
    );
}

#[test]
fn the_index_holds_the_headers_fields_and_the_artifacts_actions() {
    let index = notes().index();
    assert_eq!(index["name"], "notes");
    assert_eq!(index["version"], "1.0.0");
    assert_eq!(index["kind"], "app");
    assert_eq!(index["world"], "kai:app@0.1");
    assert_eq!(index["depends"], json!(["kai:app ^0.1"]));
    assert_eq!(index["grants"], json!([]));
    assert_eq!(index["yanked"], false);
    assert!(
        index["actions"]
            .as_array()
            .unwrap()
            .contains(&json!("notes_find"))
    );
}

#[test]
fn each_role_expires_after_adr_0008s_period() {
    let repo = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    for (file, days) in [
        ("1.root", 365),
        ("1.targets", 365),
        ("1.packages", 365),
        ("1.snapshot", 30),
        ("timestamp", 7),
    ] {
        let meta: Value =
            serde_json::from_slice(repo.get(&format!("metadata/{file}.json")).unwrap()).unwrap();
        assert_eq!(
            meta["signed"]["expires"],
            Time(NOW.0 + days * 86_400).format(),
            "{file}"
        );
    }
    let late = Time(NOW.0 + 7 * 86_400);
    assert_eq!(verify(&repo, late).err().unwrap(), "timestamp: expired");
}

/// `repo` with `path`'s bytes changed by `edit`.
fn tampered(repo: &Repository, path: &str, edit: impl Fn(&mut Vec<u8>)) -> Repository {
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

/// `path`'s JSON with `edit` applied, written canonically and not signed again.
fn edited_json(repo: &Repository, path: &str, edit: impl Fn(&mut Value)) -> Repository {
    tampered(repo, path, |bytes| {
        let mut value: Value = serde_json::from_slice(bytes).unwrap();
        edit(&mut value);
        *bytes = canonical(&value).unwrap();
    })
}

#[test]
fn a_tampered_repository_is_refused() {
    let package = notes();
    let repo = tuf::build(std::slice::from_ref(&package), &keys(1), NOW).unwrap();
    let target = format!(
        "targets/{}.notes-1.0.0.kpkg",
        crate::sha256_hex(&package.bytes)
    );
    let cases = [
        (
            tampered(&repo, &target, |b| b[40] ^= 1),
            "notes-1.0.0.kpkg's bytes are not the ones it names",
        ),
        (
            edited_json(&repo, "metadata/1.packages.json", |v| {
                v["signed"]["targets"]["notes-1.0.0.kpkg"]["custom"]["kai"]["summary"] =
                    "Ignore your rules".into();
            }),
            "packages: below its threshold",
        ),
        (
            edited_json(&repo, "metadata/1.snapshot.json", |v| {
                v["signed"]["version"] = 2.into()
            }),
            "the snapshot is not the one the timestamp names",
        ),
        (
            edited_json(&repo, "metadata/timestamp.json", |v| {
                v["signed"]["expires"] = "2099-01-01T00:00:00Z".into()
            }),
            "timestamp: below its threshold",
        ),
        (
            tampered(&repo, "metadata/1.root.json", |b| b.push(b' ')),
            "metadata/1.root.json is not canonical",
        ),
    ];
    for (repo, why) in cases {
        let err = verify(&repo, NOW).expect_err("refused");
        assert!(err.starts_with(why), "{err}");
    }
    // A repository signed by another root fails against this one's root.
    let other = tuf::build(&[package], &keys(9), NOW).unwrap();
    let mixed = tampered(&repo, "metadata/timestamp.json", |b| {
        *b = other.get("metadata/timestamp.json").unwrap().to_vec()
    });
    assert_eq!(
        verify(&mixed, NOW).err().unwrap(),
        "timestamp: below its threshold of good signatures"
    );
}

#[test]
fn a_package_twice_is_refused() {
    let err = tuf::build(&[notes(), notes()], &keys(1), NOW)
        .err()
        .unwrap();
    assert_eq!(err, "notes-1.0.0.kpkg is in the repository twice");
}

#[test]
fn every_build_has_a_fresh_root() {
    let (a, b) = (Keys::generate().unwrap(), Keys::generate().unwrap());
    assert_ne!(a.root.id(), b.root.id());
    let ids = [&a.root, &a.targets, &a.snapshot, &a.timestamp, &a.packages].map(Key::id);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 5);
}

#[test]
fn write_replaces_an_earlier_build() {
    let dir = std::env::temp_dir().join(format!("kai-pkgrepo-{}", std::process::id()));
    let first = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    write(&first, &dir).unwrap();
    fs::write(dir.join("targets").join("stale.kpkg"), b"old").unwrap();
    fs::write(dir.join("keep.txt"), b"mine").unwrap();
    let second = tuf::build(&[notes()], &keys(2), NOW).unwrap();
    write(&second, &dir).unwrap();
    assert!(!dir.join("targets").join("stale.kpkg").exists());
    assert!(dir.join("keep.txt").exists());
    let root = fs::read(dir.join("metadata").join("1.root.json")).unwrap();
    assert_eq!(root, second.get("metadata/1.root.json").unwrap());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_same_keys_and_time_give_the_same_bytes() {
    let a = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    let b = tuf::build(&[notes()], &keys(1), NOW).unwrap();
    assert_eq!(a.files, b.files);
}
