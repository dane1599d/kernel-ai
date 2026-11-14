//! The model downloaded in the OS (P5 task 9), apart from the network: what is
//! offered on a 4 GiB and a 6 GiB guest, the one ask, the job and the hosts it
//! reaches, and the chunk list held to the role. The targets are the pinned
//! ones of `xtask/models.json`.

use comp_models::{Signed, Store, Stored};
use modelchan::Sha256;
use modelchan::fetch::{
    self, DISK_MARGIN, FIXTURE, Fit, HUGGING_FACE, Hosts, MODEL_CHUNK, Machine, PARTIAL_DIR,
    Refused, Source, ask, disk_need, guest_mib_of, offers, plan, subject,
};

const GIB: u64 = 1 << 30;
const QWEN_4B: u64 = 2_497_281_120;

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
            QWEN_4B,
            0x40,
            "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        ),
    ]
}

static EMPTY: Store = Store {
    blobs: Vec::new(),
    running: None,
    next: None,
    on_trial: false,
    rolled_back: None,
};
const HOSTS: &[Hosts] = &[HUGGING_FACE];

fn machine(memory_gib: u64) -> Machine<'static> {
    Machine {
        memory_mib: memory_gib * 1024,
        free_bytes: 8 * GIB,
        store: &EMPTY,
        partials: &[],
        hosts: HOSTS,
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
    let four = offers(&role, &machine(4));
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
    let six = offers(&role, &machine(6));
    assert_eq!(six[0].fit, Fit::Fits);
    assert_eq!(recommended(&six), Some("qwen3-4b-2507"));
    // A 2 GiB guest takes none of them, and none is recommended.
    let two = offers(&role, &machine(2));
    assert!(two.iter().all(|o| matches!(o.fit, Fit::Memory { .. })));
    assert_eq!(recommended(&two), None);
}

#[test]
fn a_stored_model_is_not_offered_again_and_disk_counts_the_margin() {
    let role = role();
    let store = Store {
        blobs: vec![Stored {
            sha256: [0x40; 32],
            length: QWEN_4B,
        }],
        ..Store::default()
    };
    let six = offers(
        &role,
        &Machine {
            store: &store,
            ..machine(6)
        },
    );
    assert_eq!(six[0].fit, Fit::Stored);
    assert_eq!(recommended(&six), Some("qwen3-1.7b"));
    // Free disk for the partial file and the store's sealed copy of it, but
    // not the margin.
    let tight = Machine {
        free_bytes: 2 * QWEN_4B + DISK_MARGIN - 1,
        ..machine(6)
    };
    assert_eq!(
        offers(&role, &tight)[0].fit,
        Fit::Disk {
            need: 2 * QWEN_4B + DISK_MARGIN,
            free: tight.free_bytes
        }
    );
    // Free disk for one copy and the margin is not enough either: the partial
    // file stays until its copy is kept.
    let one_copy = Machine {
        free_bytes: QWEN_4B + DISK_MARGIN,
        ..machine(6)
    };
    assert!(matches!(offers(&role, &one_copy)[0].fit, Fit::Disk { .. }));
}

#[test]
fn a_whole_partial_file_needs_disk_only_for_its_copy() {
    assert_eq!(disk_need(QWEN_4B, 0), 2 * QWEN_4B + DISK_MARGIN);
    assert_eq!(disk_need(QWEN_4B, QWEN_4B), QWEN_4B + DISK_MARGIN);
    // A partial file longer than its target counts as whole, never as more.
    assert_eq!(disk_need(QWEN_4B, u64::MAX), QWEN_4B + DISK_MARGIN);
    assert_eq!(disk_need(u64::MAX, 0), u64::MAX);
}

#[test]
fn a_resume_needs_disk_only_for_what_is_still_to_come() {
    let role = role();
    // 2 GB of the 4B are in `.partial/`; 3.3 GB are free, enough for the rest
    // and the sealed copy.
    let held: &[(Sha256, u64)] = &[([0x40; 32], 2_000_000_000)];
    let resuming = Machine {
        free_bytes: QWEN_4B - 2_000_000_000 + QWEN_4B + DISK_MARGIN,
        partials: held,
        ..machine(6)
    };
    assert_eq!(fetch::fit(&role[2], &resuming), Fit::Fits);
    assert!(plan(&role, &resuming, "qwen3-4b-2507").is_ok());
    // Without the partial file, the same disk is too small.
    let fresh = Machine {
        partials: &[],
        ..resuming
    };
    assert!(matches!(fetch::fit(&role[2], &fresh), Fit::Disk { .. }));
    // A partial file of another target does not count.
    let other: &[(Sha256, u64)] = &[([0x17; 32], 2_000_000_000)];
    let unrelated = Machine {
        partials: other,
        ..resuming
    };
    assert!(matches!(fetch::fit(&role[2], &unrelated), Fit::Disk { .. }));
}

#[test]
fn a_target_under_another_licence_an_unpinned_source_or_another_host_is_not_offered() {
    let mut role = role();
    role[0].licence = "Llama-3.1-Community".into();
    role[1].source =
        "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf".into();
    let ids: Vec<String> = offers(&role, &machine(6))
        .iter()
        .map(|o| o.target.id.clone())
        .collect();
    assert_eq!(ids, ["qwen3-4b-2507"]);
    assert_eq!(
        plan(&role, &machine(6), "qwen3-0.6b"),
        Err(Refused::Licence("Llama-3.1-Community".into()))
    );
    assert!(matches!(
        plan(&role, &machine(6), "qwen3-1.7b"),
        Err(Refused::Source(_))
    ));
    // A source on a host no model comes from is neither offered nor planned.
    let elsewhere = Machine {
        hosts: &[],
        ..machine(6)
    };
    assert!(offers(&role, &elsewhere).is_empty());
    assert_eq!(
        plan(&role, &elsewhere, "qwen3-4b-2507"),
        Err(Refused::Source(
            "the source's host is not one models come from"
        ))
    );
}

#[test]
fn the_one_ask_names_the_model_its_size_and_the_host() {
    let role = role();
    let source = Source::parse(&role[2].source).unwrap();
    assert_eq!(
        ask(&role[2], &source),
        "Download Qwen3-4B-Instruct-2507 Q4_K_M (2.3 GiB) from huggingface.co? Each part is checked against the signed models role before it is kept."
    );
    // The menu's subject: the size and the host, the call naming the model.
    assert_eq!(
        subject(&role[2], &source),
        "Download 2.3 GiB from huggingface.co"
    );
}

#[test]
fn the_guest_reads_as_the_step_its_vm_was_sized_in() {
    const MIB: u64 = 1 << 20;
    // xtask's VM for Qwen3-4B, less a little the firmware keeps.
    assert_eq!(guest_mib_of(4160 * MIB - 10 * MIB), 4160);
    assert_eq!(guest_mib_of(4160 * MIB), 4160);
    // 4 GiB, whole or less a little, is not 4,160.
    assert_eq!(guest_mib_of(4096 * MIB), 4096);
    assert_eq!(guest_mib_of(4096 * MIB - 640 * 1024), 4096);
    assert_eq!(guest_mib_of(0), 0);
    assert_eq!(
        guest_mib_of(u64::MAX),
        u64::MAX.div_ceil(MIB).next_multiple_of(64)
    );
    // So a VM sized for the 4B by xtask is offered it, and a 4 GiB one is not.
    let role = role();
    let sized = Machine {
        memory_mib: guest_mib_of(4150 * MIB),
        ..machine(6)
    };
    assert_eq!(fetch::fit(&role[2], &sized), Fit::Fits);
    let four = Machine {
        memory_mib: guest_mib_of(4096 * MIB - 640 * 1024),
        ..machine(6)
    };
    assert!(matches!(fetch::fit(&role[2], &four), Fit::Memory { .. }));
}

#[test]
fn the_mock_hosts_stand_for_hugging_face_only_where_asked() {
    // `updated` takes FIXTURE only with the case's `fetch=test`: a role's
    // source on the mock is not offered with Hugging Face's hosts.
    let mut role = role();
    role[0].source = String::from(
        "https://models.kai.test/kernel-ai/fixture-GGUF/resolve/0123456789abcdef0123456789abcdef01234567/fixture.gguf",
    );
    let real = offers(&role, &machine(6));
    assert!(real.iter().all(|o| o.target.id != "qwen3-0.6b"));
    let mock = Machine {
        hosts: &[FIXTURE],
        ..machine(6)
    };
    let job = plan(&role, &mock, "qwen3-0.6b").unwrap();
    assert_eq!(job.hosts, ["models.kai.test", "assets.kai.test"]);
    assert!(job.reaches("assets.kai.test", 443));
    assert!(!job.reaches("x.assets.kai.test", 443));
}

#[test]
fn a_job_reaches_its_source_and_storage_hosts_on_443_and_lands_in_the_store() {
    let role = role();
    let job = plan(&role, &machine(6), "qwen3-4b-2507").unwrap();
    assert_eq!(job.source.host, "huggingface.co");
    assert_eq!(job.source.url(), role[2].source);
    let name = "40".repeat(32);
    assert_eq!(job.partial, format!("{PARTIAL_DIR}/{name}.partial"));
    assert_eq!(job.path, format!("/system/models/{name}.gguf"));
    assert_eq!(job.chunks, format!("{name}.chunks"));
    // The resolve host, and the storage host a runner saw (run 41878520332)
    // with another region's name under the same suffix.
    for host in [HUGGING_FACE.source, "us.aws.cdn.hf.co", "eu.aws.cdn.hf.co"] {
        assert!(job.reaches(host, 443), "{host}");
        assert!(!job.reaches(host, 8443), "{host}");
    }
    // Not the suffix itself, not a name that only ends the same way, not
    // other hf.co hosts the manager did not allow, and no other host.
    for host in [
        "cdn.hf.co",
        "evilcdn.hf.co",
        "cdn.hf.co.evil.test",
        "us.aws.cdn.hf.co.evil.test",
        "cdn-lfs.hf.co",
        "hf.co",
        "elsewhere.example",
        "a..cdn.hf.co",
        "-a.cdn.hf.co",
    ] {
        assert!(!job.reaches(host, 443), "{host}");
    }
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
        assert_eq!(plan(&role, &machine(4), id), Err(want.clone()), "{id}");
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
fn the_chunk_list_must_be_the_roles_target_in_2_mib_chunks() {
    let role = role();
    let job = plan(&role, &machine(6), "qwen3-4b-2507").unwrap();
    assert_eq!(job.check_list(&[0x40; 32], QWEN_4B, MODEL_CHUNK), Ok(()));
    assert_eq!(
        job.check_list(&[0x41; 32], QWEN_4B, MODEL_CHUNK),
        Err(Refused::List("the chunk list is for another blob"))
    );
    assert_eq!(
        job.check_list(&[0x40; 32], QWEN_4B + 1, MODEL_CHUNK),
        Err(Refused::List("the chunk list's size is not the target's"))
    );
    for chunk in [64 << 10, 4 << 20, 16 << 20] {
        assert_eq!(
            job.check_list(&[0x40; 32], QWEN_4B, chunk),
            Err(Refused::List("the chunk list's chunks are not 2 MiB")),
            "{chunk}"
        );
    }
}

#[test]
fn a_source_is_an_https_resolve_url_on_a_dns_name_pinned_to_a_revision() {
    let rev = "a06e946bb6b655725eafa393f4a9745d460374c9";
    let path = format!("unsloth/X-GGUF/resolve/{rev}/x.gguf");
    let source = Source::parse(&format!("https://huggingface.co/{path}")).unwrap();
    assert_eq!(source.path, format!("/{path}"));
    for host in ["a.test", "a-b.c0.test", "xn--bcher-kva.example"] {
        assert!(
            Source::parse(&format!("https://{host}/{path}")).is_ok(),
            "{host}"
        );
    }
    for host in [
        "HuggingFace.co",
        "10.0.0.1",
        "127.0.0.1",
        "a..b",
        "-a.test",
        "a-.test",
        ".test",
        "test.",
        "user@huggingface.co",
        "huggingface.co:443",
        "[::1]",
    ] {
        assert!(
            Source::parse(&format!("https://{host}/{path}")).is_err(),
            "{host}"
        );
    }
    let long = format!("{}.test", "a".repeat(64));
    assert!(Source::parse(&format!("https://{long}/{path}")).is_err());
    for bad in [
        format!("http://huggingface.co/{path}"),
        format!("https://huggingface.co/unsloth/X-GGUF/blob/{rev}/x.gguf"),
        "https://huggingface.co/unsloth/X-GGUF/resolve/main/x.gguf".into(),
        format!("https://huggingface.co/unsloth/X-GGUF/resolve/{rev}/x.bin"),
        format!("https://huggingface.co/unsloth/../resolve/{rev}/x.gguf"),
        format!("https://huggingface.co/{path}?download=1"),
        format!("https://huggingface.co/a/b/c/resolve/{rev}/x.gguf"),
        "https://huggingface.co".into(),
    ] {
        assert!(Source::parse(&bad).is_err(), "{bad}");
    }
}
