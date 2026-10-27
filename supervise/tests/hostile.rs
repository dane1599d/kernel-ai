//! Hostile plans for the `dir=` key (ADR 0011 §12): a plan line's folder grant is
//! authority, so a mutated line must never grant a folder its text does not
//! name exactly. Each mutation of a plan with `dir=` lines parses or is refused
//! line by line, never panics, and any service that parses with a folder has
//! `dir=pkg` as a word of its own line.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{fuzz, truncations};
use supervise::{Folder, parse_plan};

const PLAN: &str = "fs caps=serve+partition\n\
    pkg dir=pkg pages=2048 step=install\n\
    shell caps=call+files uses=fs dir=pkg\n\
    notes caps=serve\n";

/// Every service that came out with a folder names `dir=pkg` on its own line.
fn grants_only_what_it_says(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let plan = parse_plan(text);
    for service in &plan.services {
        if service.dirs.is_empty() {
            continue;
        }
        assert_eq!(service.dirs, [Folder::Pkg], "{text:?}");
        let line = text
            .lines()
            .find(|l| l.split_whitespace().next() == Some(service.name.as_str()))
            .expect("a parsed service has its line");
        assert!(line.split_whitespace().any(|w| w == "dir=pkg"), "{line:?}");
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
