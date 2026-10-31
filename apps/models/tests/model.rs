//! The model manager's model on the host: the list, the import rule and the
//! intents each choice gives.

use comp_models::{
    Disk, Import, Intent, MIN_PREFIX, Models, Refusal, Sha256, Signed, State, Store, Stored, check,
    could_be, hex, size,
};

const SMALL: Sha256 = [0x11; 32];
const LARGE: Sha256 = [0x22; 32];
const AWAY: Sha256 = [0x33; 32];
const DEV: Sha256 = [0x44; 32];

fn signed(id: &str, length: u64, sha256: Sha256) -> Signed {
    Signed {
        id: id.into(),
        name: format!("{id} model"),
        length,
        sha256,
        licence: "Apache-2.0".into(),
        source: format!("example/{id}@0123abcd"),
    }
}

/// The role signs three models; the store holds two of them and one blob xtask
/// put there on the host, which the role does not name.
fn models() -> Models {
    Models {
        signed: vec![
            signed("small", 1000, SMALL),
            signed("large", 5000, LARGE),
            signed("away", 7000, AWAY),
        ],
        store: Store {
            blobs: vec![
                Stored {
                    sha256: SMALL,
                    length: 1000,
                },
                Stored {
                    sha256: LARGE,
                    length: 5000,
                },
                Stored {
                    sha256: DEV,
                    length: 3000,
                },
            ],
            running: Some(SMALL),
            ..Store::default()
        },
        disks: vec![
            Disk {
                id: 4,
                label: "kai-model".into(),
                length: 7000,
            },
            Disk {
                id: 5,
                label: "photos".into(),
                length: 123,
            },
        ],
        import: None,
    }
}

#[test]
fn check_takes_a_signed_blob_and_refuses_the_rest() {
    let m = models();
    assert_eq!(check(&m.signed, 5000, &LARGE).map(|s| &*s.id), Ok("large"));
    assert_eq!(
        check(&m.signed, 5000, &DEV),
        Err(Refusal::Hash { id: "large".into() })
    );
    assert_eq!(check(&m.signed, 3000, &DEV), Err(Refusal::NotSigned));
    // The right hash at another length is not the target either.
    assert_eq!(
        check(&m.signed, 1000, &LARGE),
        Err(Refusal::Hash { id: "small".into() })
    );
    let mut gpl = m.signed.clone();
    gpl[1].licence = "GPL-3.0-only".into();
    assert_eq!(
        check(&gpl, 5000, &LARGE),
        Err(Refusal::Licence("GPL-3.0-only".into()))
    );
    assert!(could_be(&m.signed, 7000) && !could_be(&m.signed, 7001));
}

#[test]
fn check_never_takes_a_hash_the_role_does_not_sign() {
    let m = models();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..5000 {
        let mut sha = [0u8; 32];
        for b in &mut sha {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        let length = [1000, 5000, 7000, x % 10_000][(x >> 8) as usize % 4];
        if m.signed.iter().any(|s| s.sha256 == sha) {
            continue;
        }
        assert!(check(&m.signed, length, &sha).is_err(), "{}", hex(&sha));
    }
}

#[test]
fn the_list_has_each_model_once_by_state_with_its_licence() {
    let mut m = models();
    m.store.next = Some(LARGE);
    m.store.blobs.push(Stored {
        sha256: SMALL,
        length: 1000,
    });
    let rows = m.rows();
    let seen: Vec<(State, String)> = rows.iter().map(|r| (r.state, r.name())).collect();
    assert_eq!(
        seen,
        [
            (State::Running, "small model".into()),
            (State::Next, "large model".into()),
            (State::Stored, format!("sha256 {}", "44".repeat(6))),
            (State::Available, "away model".into()),
        ]
    );
    assert_eq!(rows[0].licence(), "Apache-2.0");
    assert_eq!(rows[2].licence(), "not in the signed role");
}

#[test]
fn use_switches_to_a_stored_model_and_back() {
    let mut m = models();
    // Not stored, or what the next restart uses already: nothing to do.
    assert_eq!(m.use_model(&AWAY), None);
    assert_eq!(m.use_model(&SMALL), None);
    assert_eq!(m.use_model(&[0; 32]), None);
    let switch = m.use_model(&LARGE).unwrap();
    assert_eq!(switch, Intent::Use(LARGE));
    m.applied(&switch);
    assert_eq!(m.next(), Some(LARGE));
    assert_eq!(m.row(&LARGE).unwrap().state, State::Next);
    // Back to the running model, as `undo` does: the next restart keeps it.
    let back = m.use_model(&SMALL).unwrap();
    m.applied(&back);
    assert_eq!((m.store.next, m.next()), (None, Some(SMALL)));
    // A stored blob the role does not name may be used; one the role puts
    // under a licence the project refuses may not.
    assert_eq!(m.use_model(&DEV), Some(Intent::Use(DEV)));
    m.signed[1].licence = "CC-BY-NC-4.0".into();
    assert_eq!(m.use_model(&LARGE), None);
}

#[test]
fn an_import_needs_a_disk_of_a_signed_size_and_reports_its_end() {
    let mut m = models();
    assert_eq!(m.import(5), None, "no target is 123 bytes");
    assert_eq!(m.import(9), None, "no such disk");
    let import = m.import(4).unwrap();
    assert_eq!(import, Intent::Import(4));
    m.applied(&import);
    assert_eq!(m.import(4), None, "one import at a time");
    m.progress(9999);
    assert_eq!(
        m.import,
        Some(Import::Copying {
            disk: 4,
            done: 7000,
            length: 7000
        })
    );
    m.imported(Ok(Stored {
        sha256: AWAY,
        length: 7000,
    }));
    assert_eq!(
        m.import,
        Some(Import::Stored {
            disk: 4,
            sha256: AWAY
        })
    );
    assert_eq!(m.row(&AWAY).unwrap().state, State::Stored);
    assert_eq!(m.use_model(&AWAY), Some(Intent::Use(AWAY)));
    // A result with no import running changes nothing.
    let before = m.clone();
    m.imported(Err(Refusal::NotGguf));
    assert_eq!(m, before);

    m.applied(&Intent::Import(4));
    m.imported(Err(Refusal::Hash { id: "away".into() }));
    let Some(Import::Refused { disk: 4, why }) = &m.import else {
        panic!("{:?}", m.import);
    };
    assert!(why.why().contains("away"), "{}", why.why());
    assert_eq!(m.store.blobs.len(), 4);
}

#[test]
fn find_takes_ids_names_and_one_models_hash_prefix() {
    let mut m = models();
    let id = |r: Option<comp_models::Row<'_>>| r.map(|r| r.sha256);
    assert_eq!(id(m.find("LARGE")), Some(LARGE));
    assert_eq!(id(m.find("away model")), Some(AWAY));
    assert_eq!(id(m.find(&"44".repeat(4))), Some(DEV));
    assert_eq!(id(m.find(&"44".repeat(32))), Some(DEV));
    assert_eq!(id(m.find(&"4".repeat(MIN_PREFIX - 1))), None, "too short");
    assert_eq!(id(m.find("4444444z")), None, "not hex");
    assert_eq!(id(m.find("nothing")), None);
    let mut twin = [0x44; 32];
    twin[31] = 0;
    m.store.blobs.push(Stored {
        sha256: twin,
        length: 3000,
    });
    assert_eq!(id(m.find(&"44".repeat(4))), None, "two models begin so");
}

#[test]
fn sizes_read_for_people() {
    assert_eq!(size(0), "0 bytes");
    assert_eq!(size(1023), "1023 bytes");
    assert_eq!(size(1536), "1.5 KiB");
    assert_eq!(size(639_446_688), "609.8 MiB");
    assert_eq!(size(2_497_280_256), "2.3 GiB");
    assert_eq!(size(u64::MAX), "17179869183.9 GiB");
}
