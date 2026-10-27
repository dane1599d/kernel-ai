use super::*;
use alloc::vec;
use serde_json::json;

fn envelope(signed: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "signed": signed, "signatures": [] })).unwrap()
}

fn root() -> Value {
    let key = json!({ "keytype": "ed25519", "scheme": "ed25519", "keyval": { "public": "00".repeat(32) } });
    let role = json!({ "keyids": ["k"], "threshold": 1 });
    json!({
        "_type": "root", "spec_version": "1.0.31", "version": 1, "expires": "2030-01-01T00:00:00Z",
        "consistent_snapshot": true, "keys": { "k": key },
        "roles": { "root": role, "timestamp": role, "snapshot": role, "targets": role },
    })
}

#[test]
fn a_root_parses_and_keeps_unknown_fields_in_what_is_signed() {
    let mut signed = root();
    signed["custom-field"] = json!("value");
    let meta = Root::parse(&envelope(signed.clone())).unwrap();
    assert_eq!(meta.version, 1);
    assert_eq!(meta.expires, 1_893_456_000);
    assert!(meta.signed.consistent_snapshot);
    assert_eq!(meta.signed.role("timestamp").keyids, ["k"]);
    assert_eq!(meta.canonical, canonical(&signed).unwrap());
}

#[test]
fn malformed_roots_are_refused() {
    let cases: Vec<fn(&mut Value)> = vec![
        |v| _ = v.as_object_mut().unwrap().remove("version"),
        |v| v["version"] = json!(0),
        |v| v["version"] = json!(true),
        |v| v["version"] = json!(1.5),
        |v| v["_type"] = json!("timestamp"),
        |v| v["spec_version"] = json!("2.0.0"),
        |v| v["spec_version"] = json!("1"),
        |v| v["spec_version"] = json!("1.x.0"),
        |v| v["expires"] = json!("2030-01-01T00:00:00.5Z"),
        |v| v["expires"] = json!("2030-02-30T00:00:00Z"),
        |v| v["consistent_snapshot"] = json!("yes"),
        |v| _ = v["roles"].as_object_mut().unwrap().remove("snapshot"),
        |v| v["roles"]["mirrors"] = json!({ "keyids": [], "threshold": 1 }),
        |v| v["roles"]["root"]["keyids"] = json!(["k", "k"]),
        |v| v["roles"]["root"]["keyids"] = json!(["missing"]),
        |v| v["roles"]["root"]["threshold"] = json!(0),
        |v| v["keys"]["k"] = json!({ "keytype": "ed25519" }),
        |v| v["note"] = json!(0.5),
    ];
    for (i, case) in cases.iter().enumerate() {
        let mut signed = root();
        case(&mut signed);
        assert!(Root::parse(&envelope(signed)).is_err(), "case {i}");
    }
    assert!(Root::parse(b"[1]").is_err());
    assert!(Root::parse(b"{\"signed\": {}, \"signatures\": {}}").is_err());
}

#[test]
fn times_are_whole_utc_seconds() {
    assert_eq!(time("1970-01-01T00:00:00Z"), Ok(0));
    assert_eq!(time("2000-03-01T12:34:56Z"), Ok(951_914_096));
    assert_eq!(time("2024-02-29T00:00:00Z"), Ok(1_709_164_800));
    for bad in [
        "2023-02-29T00:00:00Z",
        "1969-12-31T23:59:59Z",
        "2030-01-01 00:00:00Z",
        "2030-01-01T24:00:00Z",
        "2030-1-01T00:00:00Z",
    ] {
        assert!(time(bad).is_err(), "{bad}");
    }
}

#[test]
fn targets_delegate_by_pattern_or_hash_prefix() {
    let key = json!({ "keytype": "ed25519", "scheme": "ed25519", "keyval": { "public": "00".repeat(32) } });
    let signed = json!({
        "_type": "targets", "spec_version": "1.0", "version": 3, "expires": "2030-01-01T00:00:00Z",
        "targets": { "a/b.txt": { "length": 5, "hashes": { "sha256": "00" }, "custom": { "kai": 1 } } },
        "delegations": { "keys": { "k": key }, "roles": [
            { "name": "docs", "keyids": ["k"], "threshold": 1, "terminating": true, "paths": ["*.md", "foo-?.tgz"] },
            { "name": "bins", "keyids": ["k"], "threshold": 1, "terminating": false, "path_hash_prefixes": ["8"] },
        ] },
    });
    let meta = Targets::parse(&envelope(signed)).unwrap();
    let target = &meta.signed.targets["a/b.txt"];
    assert_eq!((target.length, target.custom["kai"].clone()), (5, json!(1)));
    let roles = &meta.signed.delegations.as_ref().unwrap().roles;
    assert!(roles[0].terminating && roles[0].matches("README.md") && roles[0].matches("foo-1.tgz"));
    assert!(
        !roles[0].matches("x/README.md")
            && !roles[0].matches("foo-12.tgz")
            && !roles[0].matches("README.MD")
    );
    // sha256("x") is 2d71..., sha256("b") is 3e23..., sha256("a") is ca97...
    let bins = DelegatedRole {
        paths: Paths::HashPrefixes(vec!["2d".into(), "3e".into()]),
        ..roles[1].clone()
    };
    assert!(bins.matches("x") && bins.matches("b") && !bins.matches("a"));
}

#[test]
fn bad_delegations_are_refused() {
    let key = json!({ "keytype": "ed25519", "scheme": "ed25519", "keyval": { "public": "00" } });
    let role = |extra: Value| {
        let mut r = json!({ "name": "r", "keyids": ["k"], "threshold": 1, "terminating": false });
        r.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        r
    };
    let cases = [
        json!([role(json!({}))]),
        json!([role(json!({ "paths": ["*"], "path_hash_prefixes": ["0"] }))]),
        json!([role(json!({ "paths": ["*"], "name": "targets" }))]),
        json!([
            role(json!({ "paths": ["*"] })),
            role(json!({ "paths": ["*"] }))
        ]),
        json!([role(json!({ "paths": ["*"], "terminating": "no" }))]),
    ];
    for (i, roles) in cases.into_iter().enumerate() {
        let signed = json!({
            "_type": "targets", "spec_version": "1.0", "version": 1, "expires": "2030-01-01T00:00:00Z",
            "targets": {}, "delegations": { "keys": { "k": key }, "roles": roles },
        });
        assert!(Targets::parse(&envelope(signed)).is_err(), "case {i}");
    }
    let succinct = json!({
        "_type": "targets", "spec_version": "1.0", "version": 1, "expires": "2030-01-01T00:00:00Z",
        "targets": {}, "delegations": { "keys": {}, "succinct_roles": {} },
    });
    assert!(Targets::parse(&envelope(succinct)).is_err());
}

#[test]
fn patterns_match_segment_by_segment_as_fnmatchcase_does() {
    for (pattern, path, hit) in [
        ("*", "file", true),
        ("*", "dir/file", false),
        ("releases/*/*", "releases/x/y", true),
        ("releases/*/*", "releases/x", false),
        ("*.doc", "a.doc", true),
        ("?", "", false),
        ("[ab]x", "bx", true),
        ("[!ab]x", "bx", false),
        ("[a-c]", "c", true),
        ("[]]", "]", true),
        ("[x", "[x", true),
        ("a*b*c", "aXbYc", true),
        ("a*b*c", "aXbY", false),
    ] {
        assert_eq!(path_matches(pattern, path), hit, "{pattern} {path}");
    }
}

#[test]
fn hashes_must_be_known_and_match() {
    let file = MetaFile {
        version: 1,
        length: Some(3),
        hashes: Some(vec![("sha512".into(), Sha512::digest(b"abc").to_vec())]),
    };
    assert_eq!(file.check(b"abc", "x"), Ok(()));
    assert!(matches!(file.check(b"abd", "x"), Err(Error::Hash(_))));
    assert!(matches!(file.check(b"abcd", "x"), Err(Error::Hash(_))));
    let unknown = vec![("md5".to_string(), vec![0u8; 16])];
    assert!(
        check_hashes(&unknown, b"abc", "x")
            .unwrap_err()
            .to_string()
            .contains("unknown hash algorithm md5")
    );
}
