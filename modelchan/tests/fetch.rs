//! The model downloaded in the OS (P5 task 9), apart from the network: what is
//! offered on a 4 GiB and a 6 GiB guest, the one ask, the job, and the chunk
//! list held to the role. The targets are the pinned ones of
//! `xtask/models.json`.

use comp_models::{Signed, Store, Stored};
use modelchan::fetch::{
    self, DISK_MARGIN, Fit, Machine, PARTIAL_DIR, Refused, Source, ask, offers, plan,
};

const GIB: u64 = 1 << 30;

fn signed(id: &str, name: &str, length: u64, sha: u8, source: &str) -> Signed {
    Signed {
        id: id.into(),
        name: name.into(),
        length,
        sha256: [sha; 32],
        licence: "Apache-2.0".into(),
        source: source.into(),
    }
}

/// The role's targets as `xtask/models.json` pins them.
fn role() -> Vec<Signed> {
    vec![
        signed(
            "qwen3-0.6b",
            "Qwen3-0.6B Q8_0",
            639_446_688,
            0x06,
            "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/23749fefcc72300e3a2ad315e1317431b06b590a/Qwen3-0.6B-Q8_0.gguf",
        ),
        signed(
            "qwen3-1.7b",
            "Qwen3-1.7B Q4_K_M",
            1_107_409_472,
            0x17,
            "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/d7f544eead698dbd1f15126ef60b45a1e1933222/Qwen3-1.7B-Q4_K_M.gguf",
        ),
        signed(
            "qwen3-4b-2507",
            "Qwen3-4B-Instruct-2507 Q4_K_M",
            2_497_281_120,
            0x40,
            "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        ),
    ]
}

fn machine(memory_gib: u64) -> Machine {
    Machine {
        memory_mib: memory_gib * 1024,
        free_bytes: 8 * GIB,
    }
}

fn recommended<'a>(offers: &'a [fetch::Offer<'a>]) -> Option<&'a str> {
    offers
        .iter()
        .find(|o| o.recommended)
        .map(|o| o.target.id.as_str())
}

#[test]
fn a_4_gib_guest_is_offered_the_1_7b_and_a_6_gib_one_the_4b() {
    let role = role();
    let store = Store::default();
    let four = offers(&role, &store, &machine(4));
    let ids: Vec<&str> = four.iter().map(|o| o.target.id.as_str()).collect();
    assert_eq!(
        ids,
        ["qwen3-4b-2507", "qwen3-1.7b", "qwen3-0.6b"],
        "largest first"
    );
    assert_eq!(
        four[0].fit,
        Fit::Memory {
            need_mib: 4160,
            have_mib: 4096
        }
    );
    assert_eq!(recommended(&four), Some("qwen3-1.7b"));
    let six = offers(&role, &store, &machine(6));
    assert_eq!(six[0].fit, Fit::Fits);
    assert_eq!(recommended(&six), Some("qwen3-4b-2507"));
    // A 2 GiB guest takes none of them, and none is recommended.
    let two = offers(&role, &store, &machine(2));
    assert!(two.iter().all(|o| matches!(o.fit, Fit::Memory { .. })));
    assert_eq!(recommended(&two), None);
}

#[test]
fn a_stored_model_is_not_offered_again_and_disk_counts_the_margin() {
    let role = role();
    let store = Store {
        blobs: vec![Stored {
            sha256: [0x40; 32],
            length: 2_497_281_120,
        }],
        ..Store::default()
    };
    let six = offers(&role, &store, &machine(6));
    assert_eq!(six[0].fit, Fit::Stored);
    assert_eq!(recommended(&six), Some("qwen3-1.7b"));
    // Free disk for the target alone, but not its margin.
    let tight = Machine {
        memory_mib: 6 * 1024,
        free_bytes: 2_497_281_120 + DISK_MARGIN - 1,
    };
    let fit = offers(&role, &Store::default(), &tight)[0].fit;
    assert_eq!(
        fit,
        Fit::Disk {
            need: 2_497_281_120 + DISK_MARGIN,
            free: tight.free_bytes
        }
    );
}

#[test]
fn a_target_under_another_licence_or_with_an_unpinned_source_is_not_offered() {
    let mut role = role();
    role[0].licence = "Llama-3.1-Community".into();
    role[1].source =
        "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf".into();
    let ids: Vec<String> = offers(&role, &Store::default(), &machine(6))
        .iter()
        .map(|o| o.target.id.clone())
        .collect();
    assert_eq!(ids, ["qwen3-4b-2507"]);
    assert_eq!(
        plan(&role, &Store::default(), &machine(6), "qwen3-0.6b"),
        Err(Refused::Licence("Llama-3.1-Community".into()))
    );
    assert!(matches!(
        plan(&role, &Store::default(), &machine(6), "qwen3-1.7b"),
        Err(Refused::Source(_))
    ));
}

#[test]
fn the_one_ask_names_the_model_its_size_and_the_host() {
    let role = role();
    let source = Source::parse(&role[2].source).unwrap();
    assert_eq!(
        ask(&role[2], &source),
        "Download Qwen3-4B-Instruct-2507 Q4_K_M (2.3 GiB) from huggingface.co? Each part is checked against the signed models role before it is kept."
    );
}

#[test]
fn a_job_fetches_the_pinned_source_into_the_partial_folder_then_the_store() {
    let role = role();
    let job = plan(&role, &Store::default(), &machine(6), "qwen3-4b-2507").unwrap();
    assert_eq!(job.source.host, "huggingface.co");
    assert_eq!(job.source.url(), role[2].source);
    let name = "40".repeat(32);
    assert_eq!(job.partial, format!("{PARTIAL_DIR}/{name}.gguf"));
    assert_eq!(job.path, format!("/system/models/{name}.gguf"));
    assert_eq!(job.chunks, format!("{name}.chunks"));
    assert!(job.reaches("huggingface.co", 443));
    assert!(!job.reaches("huggingface.co", 8443));
    assert!(!job.reaches("elsewhere.example", 443));
    // Each reason not to start, in words.
    let cases = [
        ("qwen3-8b", Refused::NotSigned("qwen3-8b".into())),
        (
            "qwen3-4b-2507",
            Refused::Memory {
                need_mib: 4160,
                have_mib: 4096,
            },
        ),
    ];
    for (id, want) in cases {
        let got = plan(&role, &Store::default(), &machine(4), id);
        assert_eq!(got, Err(want.clone()), "{id}");
        assert!(!want.why().is_empty());
    }
    assert_eq!(
        Refused::Memory {
            need_mib: 4160,
            have_mib: 4096
        }
        .why(),
        "it needs 4160 MiB of memory and this machine has 4096 MiB"
    );
}

#[test]
fn the_chunk_list_must_be_the_roles_target() {
    let role = role();
    let job = plan(&role, &Store::default(), &machine(6), "qwen3-4b-2507").unwrap();
    assert_eq!(job.check_list(&[0x40; 32], 2_497_281_120), Ok(()));
    assert_eq!(
        job.check_list(&[0x41; 32], 2_497_281_120),
        Err(Refused::List("the chunk list is for another blob"))
    );
    assert_eq!(
        job.check_list(&[0x40; 32], 2_497_281_121),
        Err(Refused::List("the chunk list's size is not the target's"))
    );
}

#[test]
fn a_source_is_an_https_resolve_url_pinned_to_a_revision() {
    let rev = "a06e946bb6b655725eafa393f4a9745d460374c9";
    let good = format!("https://huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.gguf");
    let source = Source::parse(&good).unwrap();
    assert_eq!(source.path, format!("/unsloth/X-GGUF/resolve/{rev}/x.gguf"));
    for bad in [
        format!("http://huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.gguf"),
        format!("https://HuggingFace.co/unsloth/X-GGUF/resolve/{rev}/x.gguf"),
        format!("https://huggingface.co/unsloth/X-GGUF/blob/{rev}/x.gguf"),
        "https://huggingface.co/unsloth/X-GGUF/resolve/main/x.gguf".into(),
        format!("https://huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.bin"),
        format!("https://huggingface.co/unsloth/../resolve/{rev}/x.gguf"),
        format!("https://huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.gguf?download=1"),
        format!("https://user@huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.gguf"),
        format!("https://huggingface.co/a/b/c/resolve/{rev}/x.gguf"),
        "https://huggingface.co".into(),
    ] {
        assert!(Source::parse(&bad).is_err(), "{bad}");
    }
}
