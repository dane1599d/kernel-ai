//! The bench's fixtures (`fixtures/`) are the test CA's, made again byte for
//! byte from their seeds. `KAINET_WRITE_FIXTURES=1` writes them anew.

use kainet::testca::{Ca, Days, Key, KeyKind};

const VALID: Days = ((2025, 1, 1), (2045, 1, 1));

fn made() -> [(&'static str, Vec<u8>); 3] {
    let ca = Ca::new(
        "kainet bench CA",
        Key::new(KeyKind::P256, [0x21; 32]).unwrap(),
        VALID,
    )
    .unwrap();
    let key = Key::new(KeyKind::P256, [0x22; 32]).unwrap();
    let leaf = ca.issue(&[kainet::bench::NAME], &key, VALID).unwrap();
    [
        ("bench-ca.der", ca.der().to_vec()),
        ("bench-leaf.der", leaf),
        ("bench-leaf.pk8", key.pkcs8().unwrap()),
    ]
}

#[test]
fn the_bench_fixtures_are_the_test_cas() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let write = std::env::var_os("KAINET_WRITE_FIXTURES").is_some();
    for (name, bytes) in made() {
        let path = dir.join(name);
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &bytes).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
    }
}
