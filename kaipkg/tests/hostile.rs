//! Hostile `custom.kai` index entries (P4 plan §1, host gates): the image check's
//! entry parser never panics, and an entry it takes says what the package's own
//! header says.

#[path = "../../testkit/hostile.rs"]
mod hostile;
#[path = "hostile/store.rs"]
mod store;

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
        for key in ["grants", "actions", "effects", "yanked"] {
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

/// A good input for each of the `kaipkg_store` body's readers, the reader's
/// byte first.
fn store_samples() -> Vec<Vec<u8>> {
    let generation = "kai-gen 1\nparent 41\ntime 2026-11-02T10:31:00Z\nwhy actlog 4812\n\
        pkg notes 2.1.0 9f42000000000000000000000000000000000000000000000000000000008b1d explicit grants 17 18\n\
        pkg spell-en 1.2.3 1c0e0000000000000000000000000000000000000000000000000000000077aa dep\n";
    let catalog = "kai-catalog 1\n\
        installed\tnotes\t1.0.0\tapp\t74510\tMPL-2.0\t-\t-\tPlain-text notes the AI can read and write\n\
        mirror\tbig\t1.2.0\tdata\t307300\tMIT OR Apache-2.0\tfiles,net:dns\t1.0.0\tA test package\n";
    let mut app = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    livecore::artifact::append_custom_section(
        &mut app,
        kaipkg::local::APP_SECTION,
        b"kai-app 1\nid notes\nabi 1.0\n",
    );
    let with = |which: u8, bytes: &[u8]| [&[which][..], bytes].concat();
    vec![
        with(store::GENERATION, generation.as_bytes()),
        with(store::TIME, b"2026-11-02T10:31:00Z"),
        with(store::CATALOG, catalog.as_bytes()),
        with(store::STORE, generation.as_bytes()),
        with(store::STORE, b"41\n"),
        with(store::LOCAL_APP, &[&[5u8][..], b"notes", &app].concat()),
    ]
}

/// The `kaipkg_store` fuzz body (P6 G41) on mutations and truncations of
/// each record `pkg` reads from the person's own disk: each is refused or
/// read, never a panic, and what is read writes back as the same text.
#[test]
fn the_store_body_refuses_or_reads_back_hostile_records() {
    let samples = store_samples();
    for sample in &samples {
        store::kaipkg_store(sample);
    }
    // Each sample is one its reader takes, so the mutations start from inside.
    let text = |s: &Vec<u8>| core::str::from_utf8(&s[1..]).unwrap().to_string();
    assert!(kaipkg::generation::Generation::parse(&text(&samples[0])).is_ok());
    assert!(kaipkg::generation::Time::parse(&text(&samples[1])).is_ok());
    assert!(kaipkg::catalog::parse(&text(&samples[2])).is_ok());
    assert!(kaipkg::local::is_app(&samples[5][7..], "notes").is_ok());
    let cases = if cfg!(debug_assertions) {
        3_000
    } else {
        30_000
    };
    for (seed, sample) in samples.iter().enumerate() {
        hostile::fuzz(sample, 0x41_0000 + seed as u64, cases, store::kaipkg_store);
        hostile::truncations(sample, |_, bytes| store::kaipkg_store(bytes));
    }
    store::kaipkg_store(&[]);
}
