//! Hostile `custom.kai` index entries (P4 plan §1, host gates): the image check's
//! entry parser never panics, and an entry it takes says what the package's own
//! header says.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaipkg::bundle;
use kaipkg::image::entry;
use kaituf::meta::TargetFile;
use serde_json::{Value, json};

const HEADER: &str = "kai-package 1\nname hello\nversion 1.0.0\nkind app\nsummary A greeting\n\
    licence MIT\nworld kai:app@0.1\ndepends kai:app ^0.1\n";
const INDEX: &str = r#"{"actions":["hello_greet"],"depends":["kai:app ^0.1"],"grants":[],"kind":"app","licence":"MIT","name":"hello","summary":"A greeting","version":"1.0.0","world":"kai:app@0.1","yanked":false}"#;

/// `kai` without the fields the header does not hold.
fn header_fields(mut kai: Value) -> Value {
    if let Some(map) = kai.as_object_mut() {
        for key in ["grants", "actions", "yanked"] {
            map.remove(key);
        }
    }
    kai
}

#[test]
fn hostile_index_entries_never_panic_or_differ_from_the_header() {
    let bytes = bundle::build(&[(bundle::HEADER, HEADER.as_bytes())]);
    let target = |kai: Value| TargetFile {
        path: "hello-1.0.0.kpkg".into(),
        length: bytes.len() as u64,
        hashes: vec![("sha256".into(), kaipkg::sha256(&bytes).to_vec())],
        custom: json!({ "kai": kai }),
    };
    let index: Value = serde_json::from_str(INDEX).unwrap();
    assert!(entry(&target(index.clone()), &bytes).is_ok());
    let mut taken = 0;
    let mut check = |text: &[u8]| {
        let Ok(kai) = serde_json::from_slice::<Value>(text) else {
            return;
        };
        if entry(&target(kai.clone()), &bytes).is_ok() {
            assert_eq!(header_fields(kai), header_fields(index.clone()));
            taken += 1;
        }
    };
    hostile::fuzz(INDEX.as_bytes(), 0x6b61_6970_6b67, 5_000, &mut check);
    hostile::truncations(INDEX.as_bytes(), |_, text| check(text));
    assert!(taken > 0, "the fuzz never kept a parseable entry");
}
