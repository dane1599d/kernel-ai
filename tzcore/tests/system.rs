//! The host's own tz data, where it has some (`/usr/share/zoneinfo` on Linux
//! and macOS): every TZif file in it parses, and a few zones show the changes
//! the other tests build by hand. Skipped, saying so, where there is none.

mod common;

use common::utc;
use std::fs;
use std::path::{Path, PathBuf};
use tzcore::{Resolved, Zone};

const ROOT: &str = "/usr/share/zoneinfo";

fn zone(name: &str) -> Option<Zone> {
    let bytes = fs::read(Path::new(ROOT).join(name)).ok()?;
    Some(Zone::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}")))
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => files(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

#[test]
fn every_tzif_file_on_this_host_parses() {
    let mut all = Vec::new();
    files(Path::new(ROOT), &mut all);
    let mut parsed = 0;
    for path in &all {
        let Ok(bytes) = fs::read(path) else { continue };
        if !bytes.starts_with(b"TZif") {
            continue;
        }
        if let Err(e) = Zone::parse(&bytes) {
            panic!("{}: {e}", path.display());
        }
        parsed += 1;
    }
    std::eprintln!("tzcore: parsed {parsed} TZif files under {ROOT}");
}

#[test]
fn the_hosts_zones_switch_where_the_built_ones_do() {
    let Some(ny) = zone("America/New_York") else {
        std::eprintln!("tzcore: no {ROOT}/America/New_York; skipped");
        return;
    };
    assert_eq!(ny.at(utc(2024, 3, 10, 6, 59)).abbr, "EST");
    assert_eq!(ny.at(utc(2024, 3, 10, 7, 0)).abbr, "EDT");
    assert_eq!(
        ny.at(utc(2006, 4, 2, 7, 0)).abbr,
        "EDT",
        "the old April rule"
    );
    assert_eq!(ny.at(utc(2006, 3, 20, 12, 0)).abbr, "EST");
    assert!(matches!(
        ny.resolve(tzcore::DateTime::date(2024, 3, 10).at(2, 30, 0)),
        Resolved::Gap { .. }
    ));
    if let Some(berlin) = zone("Europe/Berlin") {
        assert_eq!(berlin.at(utc(2024, 3, 31, 1, 0)).abbr, "CEST");
        assert_eq!(berlin.at(utc(2024, 10, 27, 1, 0)).abbr, "CET");
    }
    if let Some(sp) = zone("America/Sao_Paulo") {
        assert_eq!(sp.at(utc(2018, 12, 25, 12, 0)).offset, -2 * 3600);
        assert_eq!(sp.at(utc(2019, 11, 10, 12, 0)).offset, -3 * 3600);
        assert!(
            !sp.at(utc(2030, 1, 15, 12, 0)).dst,
            "no daylight saving after 2019"
        );
    }
    if let Some(sydney) = zone("Australia/Sydney") {
        assert_eq!(sydney.at(utc(2024, 4, 6, 15, 59)).abbr, "AEDT");
        assert_eq!(sydney.at(utc(2024, 4, 6, 16, 0)).abbr, "AEST");
    }
    if let Some(kathmandu) = zone("Asia/Kathmandu") {
        assert_eq!(
            kathmandu.at(utc(1985, 12, 31, 18, 29)).offset,
            5 * 3600 + 1800
        );
        assert_eq!(
            kathmandu.at(utc(1985, 12, 31, 18, 30)).offset,
            5 * 3600 + 2700
        );
    }
}
