//! Hostile plans for the `dir=` key (ADR 0011 §12): a plan line's folder grant is
//! authority, so a mutated line must never grant a folder its text does not
//! name exactly. Each mutation of a plan with `dir=` lines parses or is refused
//! line by line, never panics, and any service that parses with a folder has
//! `dir=pkg` as a word of its own line.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{fuzz, truncations};
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
    updated caps=update dir=trust+updates step=update\n\
    notes caps=serve dir=trust\n\
    pkg dir=pkg+updates\n\
    shell program=updated dir=updates caps=call\n";

/// The folder a `dir=` word's part names.
fn named(part: &str) -> Option<Folder> {
    Folder::parse(part)
}

/// Every folder a service came out with is named in its own line's `dir=` word,
/// and `/system/trust` and `/system/updates` go only to the program `updated`.
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
            .any(|d| matches!(d, Folder::Trust | Folder::Updates))
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
    assert_eq!(dirs("updated"), Some(vec![Folder::Trust, Folder::Updates]));
    // A line for another program asking for them is refused whole.
    assert_eq!(dirs("notes"), None);
    assert_eq!(dirs("pkg"), None);
    // Running the program `updated` under another name is refused too: the
    // name and the program must both be `updated`, as for the UPDATE capability.
    assert_eq!(dirs("shell"), None);
    assert_eq!(plan.errors.len(), 3, "{:?}", plan.errors);
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
        "updated caps=update dir=trust+updates+settings\n\
         desktop dir=settings\n",
    );
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let dirs: Vec<Vec<Folder>> = plan.services.iter().map(|s| s.dirs.clone()).collect();
    assert_eq!(
        dirs,
        vec![
            vec![Folder::Trust, Folder::Updates, Folder::Settings],
            vec![Folder::Settings],
        ]
    );
    assert!(Folder::Settings.read_only());
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
