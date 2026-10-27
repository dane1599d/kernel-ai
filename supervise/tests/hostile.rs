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
    pkg dir=pkg boot=pkgrepo pages=2048 step=install\n\
    shell caps=call+files uses=fs dir=pkg\n\
    notes caps=serve\n";

/// Every service that came out with a folder names `dir=pkg` on its own line,
/// and every one with boot files `boot=pkgrepo`.
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
        if !service.dirs.is_empty() {
            assert_eq!(service.dirs, [Folder::Pkg], "{text:?}");
            assert!(says("dir=pkg"), "{text:?}");
        }
        if let Some(boot) = service.boot {
            assert_eq!(boot, BootFiles::Pkgrepo, "{text:?}");
            assert!(says("boot=pkgrepo"), "{text:?}");
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

#[test]
fn near_misses_of_boot_pkgrepo_are_refused() {
    for word in [
        "boot=pkgrepo2",
        "boot=pkgrep",
        "boot=PKGREPO",
        "boots=pkgrepo",
        "boot=pkgrepo/",
        "boot=/pkgrepo",
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
