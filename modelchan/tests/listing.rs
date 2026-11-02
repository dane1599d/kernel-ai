//! `kai-models 1` and what `llm`'s health says it loaded.

use comp_models::{Models, Signed, Store, Stored};
use modelchan::listing::{encode, loaded, parse};

fn signed(id: &str, sha: u8, length: u64) -> Signed {
    Signed {
        id: id.into(),
        name: format!("{id} model, Q4_K_M"),
        length,
        sha256: [sha; 32],
        licence: "Apache-2.0".into(),
        source: String::new(),
    }
}

fn models() -> Models {
    Models {
        signed: vec![
            signed("qwen3-4b-2507", 0xaa, 2_497_281_120),
            signed("fixture", 0xcc, 4096),
        ],
        store: Store {
            blobs: vec![
                Stored {
                    sha256: [0xaa; 32],
                    length: 2_497_281_120,
                },
                Stored {
                    sha256: [0xdd; 32],
                    length: 77,
                },
            ],
            running: Some([0xaa; 32]),
            next: Some([0xdd; 32]),
            on_trial: true,
            rolled_back: None,
        },
        ..Models::default()
    }
}

#[test]
fn a_list_reads_back_as_written() {
    let m = models();
    assert_eq!(parse(&encode(&m)), Ok(m.clone()));
    let mut back = m;
    back.store.on_trial = false;
    back.store.rolled_back = Some(([0xdd; 32], "it never finished loading".into()));
    assert_eq!(parse(&encode(&back)), Ok(back.clone()));
    let mut either = models();
    either.signed[1].licence = "Apache-2.0 OR MIT".into();
    assert!(encode(&either).contains(" Apache-2.0_OR_MIT fixture "));
    assert_eq!(parse(&encode(&either)), Ok(either));
    let empty = Models::default();
    assert_eq!(encode(&empty), "kai-models 1\n");
    assert_eq!(parse("kai-models 1\n"), Ok(empty));
}

#[test]
fn a_malformed_list_is_refused() {
    let good = encode(&models());
    let sha = "aa".repeat(32);
    let bad = [
        good.trim_end().to_string(),
        good.replace("kai-models 1", "kai-models 2"),
        format!("{good}running {sha}\n"),
        format!("{good}bogus line\n"),
        format!("{good}stored {sha}\n"),
        format!("{good}stored {sha} -1\n"),
        format!("{good}stored {} 1\n", "AA".repeat(32)),
        format!("{good}signed {sha} 1 MIT id\n"),
        format!("{good}signed {sha} 1 MIT id  \n"),
        good.replace("on-trial", "on-holiday"),
        good.replace("on-trial", "rolled-back  "),
        String::new(),
    ];
    for text in bad {
        assert!(parse(&text).is_err(), "{text:?}");
    }
}

#[test]
fn health_says_which_model_loaded() {
    let reply = "model 2497281120 bytes fnv1a64 00000000deadbeef; local model qwen3; status state=ready tps10=85";
    assert_eq!(loaded(reply), Some((2_497_281_120, 0xdead_beef)));
    assert_eq!(
        loaded(&format!("ok {reply}")),
        Some((2_497_281_120, 0xdead_beef))
    );
    let failed = "model 4096 bytes fnv1a64 0123456789abcdef; the model cannot run: no tensors; status state=failed tps10=0";
    assert_eq!(loaded(failed), Some((4096, 0x0123_4567_89ab_cdef)));
    for none in [
        "err no model",
        "the model cannot run: x; status state=failed tps10=0",
        "model x bytes fnv1a64 00",
        "model 12 bytes sha256 00",
        "model 12 bytes fnv1a64 ",
        "model 12 bytes fnv1a64 0123456789abcdef01",
    ] {
        assert_eq!(loaded(none), None, "{none}");
    }
}
