//! UTC to local time and back over known changes, on zones built from their
//! published transition dates (`common`).

mod common;

use common::{adelaide, berlin, kathmandu, new_york, sao_paulo, sydney, tzif, utc};
use tzcore::{DateTime, Error, Resolved, Zone};

/// The abbreviation and offset in hours at `utc`.
fn at(zone: &Zone, utc: i64) -> (&str, f32) {
    let kind = zone.at(utc);
    (kind.abbr.as_str(), kind.offset as f32 / 3600.0)
}

fn wall(y: i32, m: u8, d: u8, h: u8, min: u8) -> DateTime {
    DateTime::date(y, m, d).at(h, min, 0)
}

#[test]
fn the_us_switches_in_march_and_november() {
    let ny = new_york();
    assert_eq!(at(&ny, utc(2024, 3, 10, 6, 59)), ("EST", -5.0));
    assert_eq!(at(&ny, utc(2024, 3, 10, 7, 0)), ("EDT", -4.0));
    assert_eq!(at(&ny, utc(2024, 11, 3, 5, 59)), ("EDT", -4.0));
    assert_eq!(at(&ny, utc(2024, 11, 3, 6, 0)), ("EST", -5.0));
    let (local, kind) = ny.local(utc(2024, 7, 4, 16, 0));
    assert_eq!((local, kind.dst), (wall(2024, 7, 4, 12, 0), true));
}

#[test]
fn the_old_us_rule_holds_until_the_files_last_transition_then_the_footer() {
    let ny = new_york();
    assert_eq!(ny.transitions(), 4);
    // 2006: still April to October.
    assert_eq!(at(&ny, utc(2006, 3, 20, 12, 0)), ("EST", -5.0));
    assert_eq!(at(&ny, utc(2006, 4, 2, 6, 59)).0, "EST");
    assert_eq!(at(&ny, utc(2006, 4, 2, 7, 0)).0, "EDT");
    assert_eq!(at(&ny, utc(2006, 10, 29, 6, 0)).0, "EST");
    // 2007, listed, and 2008 on, the footer: March to November.
    assert_eq!(at(&ny, utc(2007, 3, 20, 12, 0)), ("EDT", -4.0));
    assert_eq!(at(&ny, utc(2007, 11, 2, 12, 0)).0, "EDT");
    assert_eq!(at(&ny, utc(2008, 3, 9, 6, 59)).0, "EST");
    assert_eq!(at(&ny, utc(2008, 3, 9, 7, 0)).0, "EDT");
    assert_eq!(at(&ny, utc(2008, 11, 2, 6, 0)).0, "EST");
    // Before the first transition: the first type.
    assert_eq!(at(&ny, utc(1990, 7, 1, 0, 0)), ("EST", -5.0));
}

#[test]
fn the_eu_switches_on_the_last_sundays_of_march_and_october_at_0100_utc() {
    let berlin = berlin();
    for (year, march, october) in [(2024, 31, 27), (2025, 30, 26), (2026, 29, 25)] {
        assert_eq!(at(&berlin, utc(year, 3, march, 0, 59)), ("CET", 1.0));
        assert_eq!(at(&berlin, utc(year, 3, march, 1, 0)), ("CEST", 2.0));
        assert_eq!(at(&berlin, utc(year, 10, october, 0, 59)), ("CEST", 2.0));
        assert_eq!(at(&berlin, utc(year, 10, october, 1, 0)), ("CET", 1.0));
    }
    assert_eq!(
        berlin,
        Zone::from_rule("CET-1CEST,M3.5.0,M10.5.0/3").unwrap()
    );
}

#[test]
fn a_zone_that_dropped_daylight_saving_keeps_standard_time_after() {
    let sp = sao_paulo();
    assert_eq!(at(&sp, utc(2018, 11, 4, 2, 59)), ("-03", -3.0));
    assert_eq!(at(&sp, utc(2018, 11, 4, 3, 0)), ("-02", -2.0));
    assert_eq!(
        at(&sp, utc(2018, 12, 25, 12, 0)),
        ("-02", -2.0),
        "a southern summer"
    );
    assert_eq!(at(&sp, utc(2019, 2, 17, 1, 59)), ("-02", -2.0));
    assert_eq!(at(&sp, utc(2019, 2, 17, 2, 0)), ("-03", -3.0));
    // November 2019 would have been daylight saving time; it was not.
    assert_eq!(at(&sp, utc(2019, 11, 10, 12, 0)), ("-03", -3.0));
    assert_eq!(at(&sp, utc(2035, 1, 15, 12, 0)), ("-03", -3.0));
    assert!(!sp.at(utc(2035, 1, 15, 12, 0)).dst);
}

#[test]
fn the_southern_hemisphere_has_daylight_saving_over_the_new_year() {
    let sydney = sydney();
    // 2024-04-07 03:00 AEDT and 2024-10-06 02:00 AEST, both 16:00 UTC the day before.
    assert_eq!(at(&sydney, utc(2024, 4, 6, 15, 59)), ("AEDT", 11.0));
    assert_eq!(at(&sydney, utc(2024, 4, 6, 16, 0)), ("AEST", 10.0));
    assert_eq!(at(&sydney, utc(2024, 10, 5, 15, 59)), ("AEST", 10.0));
    assert_eq!(at(&sydney, utc(2024, 10, 5, 16, 0)), ("AEDT", 11.0));
    assert_eq!(
        at(&sydney, utc(2024, 7, 1, 0, 0)).0,
        "AEST",
        "winter is midyear"
    );
    // The UTC year and the local year differ here: still summer.
    let (local, kind) = sydney.local(utc(2024, 12, 31, 15, 0));
    assert_eq!(
        (local, kind.abbr.as_str()),
        (wall(2025, 1, 1, 2, 0), "AEDT")
    );
}

#[test]
fn half_and_three_quarter_hour_offsets() {
    let adelaide = adelaide();
    let (local, kind) = adelaide.local(utc(2024, 7, 1, 0, 0));
    assert_eq!(
        (local, kind.offset),
        (wall(2024, 7, 1, 9, 30), 9 * 3600 + 1800)
    );
    // 2024-10-06 02:00 ACST is 2024-10-05 16:30 UTC.
    assert_eq!(at(&adelaide, utc(2024, 10, 5, 16, 29)), ("ACST", 9.5));
    assert_eq!(at(&adelaide, utc(2024, 10, 5, 16, 30)), ("ACDT", 10.5));
    let kathmandu = kathmandu();
    assert_eq!(at(&kathmandu, utc(1985, 12, 31, 18, 29)), ("+0530", 5.5));
    assert_eq!(at(&kathmandu, utc(1985, 12, 31, 18, 30)), ("+0545", 5.75));
    let (local, _) = kathmandu.local(utc(2024, 1, 1, 0, 0));
    assert_eq!(local, wall(2024, 1, 1, 5, 45));
}

#[test]
fn a_spring_forward_time_is_a_gap_and_a_fall_back_one_comes_twice() {
    let ny = new_york();
    assert_eq!(
        ny.resolve(wall(2024, 3, 10, 2, 30)),
        Resolved::Gap {
            transition: utc(2024, 3, 10, 7, 0),
            shifted: utc(2024, 3, 10, 7, 30),
        },
        "02:30 never came; taken as 03:30 EDT"
    );
    assert_eq!(
        ny.resolve(wall(2024, 11, 3, 1, 30)),
        Resolved::Ambiguous {
            earlier: utc(2024, 11, 3, 5, 30),
            later: utc(2024, 11, 3, 6, 30),
        }
    );
    // Either edge of each.
    assert_eq!(
        ny.resolve(wall(2024, 3, 10, 1, 59)),
        Resolved::Single(utc(2024, 3, 10, 6, 59))
    );
    assert_eq!(
        ny.resolve(wall(2024, 3, 10, 3, 0)),
        Resolved::Single(utc(2024, 3, 10, 7, 0))
    );
    assert_eq!(
        ny.resolve(wall(2024, 11, 3, 0, 59)),
        Resolved::Single(utc(2024, 11, 3, 4, 59))
    );
    assert_eq!(
        ny.resolve(wall(2024, 11, 3, 2, 0)),
        Resolved::Single(utc(2024, 11, 3, 7, 0))
    );
    let repeated = ny.resolve(wall(2024, 11, 3, 1, 0));
    assert_eq!(
        repeated.compatible(),
        utc(2024, 11, 3, 5, 0),
        "the earlier, in EDT"
    );
}

#[test]
fn gaps_and_repeats_in_the_other_zones() {
    let sydney = sydney();
    assert!(matches!(
        sydney.resolve(wall(2024, 10, 6, 2, 30)),
        Resolved::Gap { .. }
    ));
    assert!(matches!(
        sydney.resolve(wall(2024, 4, 7, 2, 30)),
        Resolved::Ambiguous { .. }
    ));
    // Sao Paulo's last fall-back: 23:00-23:59 on 16 February 2019 came twice.
    assert_eq!(
        sao_paulo().resolve(wall(2019, 2, 16, 23, 30)),
        Resolved::Ambiguous {
            earlier: utc(2019, 2, 17, 1, 30),
            later: utc(2019, 2, 17, 2, 30),
        }
    );
    // Kathmandu skipped a quarter of an hour at midnight, 1986.
    assert_eq!(
        kathmandu().resolve(wall(1986, 1, 1, 0, 10)),
        Resolved::Gap {
            transition: utc(1985, 12, 31, 18, 30),
            shifted: utc(1985, 12, 31, 18, 40),
        }
    );
    assert!(matches!(
        adelaide().resolve(wall(2024, 10, 6, 2, 15)),
        Resolved::Gap { .. }
    ));
}

#[test]
fn every_local_time_of_a_year_resolves_back_to_its_moment() {
    for zone in [
        new_york(),
        berlin(),
        sydney(),
        adelaide(),
        sao_paulo(),
        kathmandu(),
    ] {
        let start = utc(2018, 6, 1, 0, 0);
        for step in 0..(2 * 365 * 24 * 4) {
            let t = start + step * 900;
            let (local, _) = zone.local(t);
            match zone.resolve(local) {
                Resolved::Single(u) => assert_eq!(u, t),
                Resolved::Ambiguous { earlier, later } => assert!(t == earlier || t == later),
                Resolved::Gap { .. } => panic!("{local:?} came, at {t}"),
            }
        }
    }
}

#[test]
fn damaged_files_are_refused_without_panicking() {
    let good = tzif(
        &[(utc(2006, 4, 2, 7, 0), 1)],
        &[(0, false, "AAA"), (3600, true, "BBB")],
        "AAA0BBB,M3.5.0,M10.5.0",
    );
    assert!(Zone::parse(&good).is_ok());
    for len in 0..good.len() {
        assert!(
            Zone::parse(&good[..len]).is_err(),
            "a prefix of {len} bytes"
        );
    }
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(Zone::parse(&bad), Err(Error::Magic));
    let mut bad = good.clone();
    bad[4] = b'9';
    assert_eq!(Zone::parse(&bad), Err(Error::Version));
    // Every single-byte change either parses or is an error; none panics.
    for i in 0..good.len() {
        for value in [0u8, 1, 0x7F, 0x80, 0xFF] {
            let mut changed = good.clone();
            changed[i] = value;
            let _ = Zone::parse(&changed);
        }
    }
    let unordered = tzif(&[(10, 0), (5, 0)], &[(0, false, "UTC")], "UTC0");
    assert_eq!(Zone::parse(&unordered), Err(Error::Order));
    let no_kind = tzif(&[(10, 3)], &[(0, false, "UTC")], "UTC0");
    assert_eq!(Zone::parse(&no_kind), Err(Error::Index));
    let bad_footer = tzif(&[], &[(0, false, "UTC")], "UTC0\nX");
    assert_eq!(Zone::parse(&bad_footer), Err(Error::Footer));
    assert_eq!(Zone::parse(&vec![0; 70_000]), Err(Error::TooBig));
}
