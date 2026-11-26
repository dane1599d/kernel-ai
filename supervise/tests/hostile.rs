//! Hostile plans for the `dir=` key (ADR 0011 §12): a plan line's folder grant is
//! authority, so a mutated line must never grant a folder its text does not
//! name exactly. Each mutation of a plan with `dir=` lines parses or is refused
//! line by line, never panics, and any service that parses with a folder has
//! `dir=pkg` as a word of its own line.

#[path = "../../testkit/hostile.rs"]
mod hostile;
#[path = "hostile/state.rs"]
mod state;

use hostile::{XorShift, fuzz, sweep_fields, truncations};
use supervise::{BootFiles, Folder, MAX_BOOT_PATH, parse_plan};

const PLAN: &str = "fs caps=serve+partition\n\
    pkg dir=pkg bootfiles=pkgrepo pages=2048 step=install\n\
    shell caps=call+files uses=fs dir=pkg\n\
    notes caps=serve\n";

/// Every service that came out with a folder names `dir=pkg` on its own line,
/// and every one with boot files `bootfiles=pkgrepo`.
fn grants_only_what_it_says(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let plan = parse_plan(text);
    for service in &plan.services {
        let says = |word: &str| {
            let line = text
                .lines()
                .find(|l| l.split_whitespace().next() == Some(service.name.as_str()))
                .expect("a parsed service has its line");
            line.split_whitespace().any(|w| w == word)
        };
        let updated_only = |d: &Folder| matches!(d, Folder::Trust | Folder::Updates);
        if service.dirs.iter().any(updated_only) {
            // A mutation that reached `updated`'s folders is the other test's.
            assert_eq!(service.program, "updated", "{text:?}");
            assert_eq!(service.name, "updated", "{text:?}");
        } else if service.dirs.contains(&Folder::Pkg) {
            assert!(says("dir=pkg"), "{text:?}");
        }
        if let Some(boot) = service.boot {
            assert_eq!(boot, BootFiles::Pkgrepo, "{text:?}");
            assert!(says("bootfiles=pkgrepo"), "{text:?}");
        }
    }
}

#[test]
fn mutated_plans_grant_no_folder_they_do_not_name() {
    fuzz(PLAN.as_bytes(), 0xd1, 5_000, grants_only_what_it_says);
    truncations(PLAN.as_bytes(), |_, cut| grants_only_what_it_says(cut));
}

#[test]
fn near_misses_of_dir_pkg_are_refused() {
    for word in [
        "dir=pkg2",
        "dir=pk",
        "dir= pkg",
        "dir=pkg+home",
        "dir=PKG",
        "dirs=pkg",
    ] {
        let plan = parse_plan(&format!("pkg {word}"));
        let granted = plan.services.iter().any(|s| !s.dirs.is_empty());
        assert!(!granted, "{word}");
    }
}

/// `updated`'s plan line (ADR 0008 §10) beside others that try for its folders.
const UPDATED_PLAN: &str = "fs caps=serve+partition\n\
    updated caps=update dir=trust+updates+restart step=update\n\
    notes caps=serve dir=trust\n\
    desktop caps=serve dir=restart\n\
    pkg dir=pkg+updates\n\
    shell program=updated dir=updates caps=call\n";

/// The folder a `dir=` word's part names.
fn named(part: &str) -> Option<Folder> {
    Folder::parse(part)
}

/// Every folder a service came out with is named in its own line's `dir=` word,
/// and `/system/trust`, `/system/updates` and `/system/state/restart` go only
/// to the program `updated`.
fn trust_and_updates_go_only_to_updated(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let plan = parse_plan(text);
    for service in &plan.services {
        if service.dirs.is_empty() {
            continue;
        }
        let line = text
            .lines()
            .find(|l| l.split_whitespace().next() == Some(service.name.as_str()))
            .expect("a parsed service has its line");
        let word: Vec<Folder> = line
            .split_whitespace()
            .filter_map(|w| w.strip_prefix("dir="))
            .flat_map(|v| v.split('+').filter_map(named).collect::<Vec<_>>())
            .collect();
        assert_eq!(service.dirs, word, "{line:?}");
        if service
            .dirs
            .iter()
            .any(|d| matches!(d, Folder::Trust | Folder::Updates | Folder::Restart))
        {
            assert_eq!(service.program, "updated", "{line:?}");
            assert_eq!(service.name, "updated", "{line:?}");
        }
    }
}

#[test]
fn near_misses_of_boot_pkgrepo_are_refused() {
    for word in [
        "bootfiles=pkgrepo2",
        "bootfiles=pkgrep",
        "bootfiles=PKGREPO",
        "boots=pkgrepo",
        "bootfiles=pkgrepo/",
        "bootfiles=/pkgrepo",
        // A program's own `boot=N` argument (filecheck's) is never the grant.
        "boot=pkgrepo",
    ] {
        let plan = parse_plan(&format!("pkg {word}"));
        assert!(plan.services.iter().all(|s| s.boot.is_none()), "{word}");
    }
}

/// A path BOOT_READ would take lies beneath `pkgrepo/`, in plain names.
fn confined(bytes: &[u8]) {
    let Ok(path) = core::str::from_utf8(bytes) else {
        return;
    };
    if !BootFiles::Pkgrepo.allows(path) {
        return;
    }
    assert!(path.len() <= MAX_BOOT_PATH, "{path:?}");
    let rest = path.strip_prefix("pkgrepo/").expect("beneath pkgrepo/");
    for name in rest.split('/') {
        assert!(!name.is_empty() && name != "." && name != "..", "{path:?}");
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        assert!(name.chars().all(plain), "{path:?}");
    }
}

#[test]
fn only_updated_gets_the_trust_and_updates_folders() {
    let plan = parse_plan(UPDATED_PLAN);
    let dirs = |name: &str| {
        plan.services
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.dirs.clone())
    };
    assert_eq!(
        dirs("updated"),
        Some(vec![Folder::Trust, Folder::Updates, Folder::Restart])
    );
    // A line for another program asking for them is refused whole.
    assert_eq!(dirs("notes"), None);
    assert_eq!(dirs("desktop"), None);
    assert_eq!(dirs("pkg"), None);
    // Running the program `updated` under another name is refused too: the
    // name and the program must both be `updated`, as for the UPDATE capability.
    assert_eq!(dirs("shell"), None);
    assert_eq!(plan.errors.len(), 4, "{:?}", plan.errors);
    fuzz(
        UPDATED_PLAN.as_bytes(),
        0xd2,
        5_000,
        trust_and_updates_go_only_to_updated,
    );
    truncations(UPDATED_PLAN.as_bytes(), |_, cut| {
        trust_and_updates_go_only_to_updated(cut)
    });
}

/// `dir=settings` reads the Settings app's files: any service may be given it,
/// and its `Dir` is read only.
#[test]
fn dir_settings_goes_to_any_service_and_only_reads() {
    let plan = parse_plan(
        "updated caps=update dir=trust+updates+restart\n\
         desktop dir=settings\n",
    );
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let dirs: Vec<Vec<Folder>> = plan.services.iter().map(|s| s.dirs.clone()).collect();
    assert_eq!(
        dirs,
        vec![
            vec![Folder::Trust, Folder::Updates, Folder::Restart],
            vec![Folder::Settings],
        ]
    );
    assert!(Folder::Settings.read_only());
    assert!(Folder::Restart.read_only());
    assert_eq!(Folder::Restart.badge(), kabi::fs::SYSTEM_RESTART);
    assert_eq!(Folder::Settings.badge(), kabi::fs::SYSTEM_SETTINGS);
    for writable in [Folder::Pkg, Folder::Trust, Folder::Updates] {
        assert!(!writable.read_only());
    }
}

#[test]
fn mutated_boot_paths_never_leave_pkgrepo() {
    const PATH: &str = "pkgrepo/targets/0a1b2c.notes-1.0.0.kpkg";
    assert!(BootFiles::Pkgrepo.allows(PATH));
    fuzz(PATH.as_bytes(), 0xb0, 5_000, confined);
    fuzz(
        b"pkgrepo/../metadata/../../kernel.lines",
        0xb1,
        5_000,
        confined,
    );
    truncations(PATH.as_bytes(), |_, cut| confined(cut));
}

// G26: the saved state and the `keep` and `profile` requests (`state.rs`).

#[test]
fn a_damaged_saved_state_gives_fewer_records_never_more_or_a_panic() {
    for (i, sample) in state::state_samples().iter().enumerate() {
        assert!(!supervise::decode(sample).is_empty(), "sample {i}");
        state::supervisor_state(sample);
        fuzz(sample, 0x5e00 + i as u64, 5_000, state::supervisor_state);
        truncations(sample, |_, cut| state::supervisor_state(cut));
        sweep_fields(sample, 0..sample.len(), |_, bytes| {
            state::supervisor_state(bytes)
        });
    }
}

#[test]
fn random_bytes_behind_the_state_magic_never_panic() {
    let mut rng = XorShift::new(0x5717);
    let words = [
        "0",
        "1",
        "2",
        "01",
        "18446744073709551615",
        "18446744073709551616",
        "-1",
        "+3",
        "x",
        " ",
        "\n",
    ];
    for _ in 0..5_000 {
        let magic = [
            "supervisor-state 5",
            "supervisor-state 4",
            "supervisor-state 3",
        ][rng.below(3)];
        let mut text = String::from(magic);
        for _ in 0..rng.below(4) {
            text.push('\n');
            text.push_str("svc");
            for _ in 0..rng.below(12) {
                text.push(' ');
                text.push_str(words[rng.below(words.len())]);
            }
        }
        state::supervisor_state(text.as_bytes());
    }
    for _ in 0..5_000 {
        let len = rng.below(64);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        state::supervisor_state(&bytes);
    }
}

#[test]
fn damaged_keep_and_profile_requests_take_only_what_they_say() {
    for (i, sample) in ["keep notes on", "keep build-1 off"].iter().enumerate() {
        state::keep_request(sample.as_bytes());
        fuzz(
            sample.as_bytes(),
            0x4b00 + i as u64,
            5_000,
            state::keep_request,
        );
        truncations(sample.as_bytes(), |_, cut| state::keep_request(cut));
    }
    for (i, sample) in [
        "profile 31 background 65536 cap",
        "profile 7 normal -",
        "profile 18446744073709551615 normal 0",
    ]
    .iter()
    .enumerate()
    {
        assert!(supervise::profile::Set::parse(sample).is_some(), "{sample}");
        state::profile_request(sample.as_bytes());
        fuzz(
            sample.as_bytes(),
            0x5000 + i as u64,
            5_000,
            state::profile_request,
        );
        truncations(sample.as_bytes(), |_, cut| state::profile_request(cut));
        sweep_fields(sample.as_bytes(), 0..sample.len(), |_, b| {
            state::profile_request(b)
        });
    }
    for (i, sample) in ["ok normal -", "ok background 4096", "err Permission"]
        .iter()
        .enumerate()
    {
        state::profile_answer(sample.as_bytes());
        fuzz(
            sample.as_bytes(),
            0x5100 + i as u64,
            5_000,
            state::profile_answer,
        );
        truncations(sample.as_bytes(), |_, cut| state::profile_answer(cut));
    }
}

#[test]
fn near_misses_of_a_keep_or_profile_request_are_refused() {
    for bad in [
        "keep notes",
        "keep notes on now",
        "keep no/tes on",
        "keep notes On",
        "keeps notes on",
    ] {
        assert_eq!(supervise::keep::parse(bad), None, "{bad}");
    }
    for bad in [
        "profile 31 normal -\n",
        "profile  31 normal -",
        "profile 31 normal +5",
        "profile 18446744073709551616 normal -",
        "profile 31 normal 00",
    ] {
        assert_eq!(supervise::profile::Set::parse(bad), None, "{bad:?}");
    }
    for bad in [
        "ok normal - cap",
        "ok normal",
        "ok  normal -",
        "ok normal 007",
    ] {
        assert!(
            supervise::profile::parse_before(31, bad).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn each_version_s_line_reads_as_its_own_layout() {
    use supervise::{Record, decode};
    let read = |text: &str| decode(text.as_bytes());
    let one = |name: &str| Record::new(name);
    // v4 has no keep: the word after stopped is the pid, never keep.
    let mut stopped = one("shell");
    stopped.stopped = true;
    stopped.pid = 1;
    stopped.started_ms = 900;
    stopped.recent = vec![5];
    assert_eq!(read("supervisor-state 4\nshell 0 0 0 1 1 900 5"), [stopped]);
    // v3 has neither: the fifth word is the pid.
    let mut gave_up = one("fs");
    gave_up.gave_up = true;
    gave_up.pid = 1;
    gave_up.started_ms = 1;
    assert_eq!(read("supervisor-state 3\nfs 0 0 1 1 1"), [gave_up]);
    // Only the word 1 sets a flag; a missing word or a bad number drops the line.
    let mut kept = one("notes");
    kept.pid = 3;
    assert_eq!(
        read("supervisor-state 5\nnotes 0 0 2 01 true 3 0\nshort 0 0 0 0 0 0\nbad 0 x 0 0 0 0 0"),
        [kept]
    );
    for text in [
        "supervisor-state 4\nshell 0 0 0 1 1 900 5",
        "supervisor-state 3\nfs 0 0 1 1 1",
        "supervisor-state 5\nnotes 0 0 2 01 true 3 0\nshort 0 0 0 0 0 0",
    ] {
        state::supervisor_state(text.as_bytes());
    }
}
