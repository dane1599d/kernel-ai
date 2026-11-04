//! The small parts: terms, f16, the time words and vector helpers.

use super::NOW_NS;
use crate::embed::{dot, unit};
use crate::half::{narrow, widen};
use crate::since::{DAY_NS, MAX_DAYS, resolve};
use crate::terms::{MAX_TERM, counted, words};
use alloc::string::String;

#[test]
fn terms_are_lower_cased_runs_of_letters_and_digits_of_two_or_more() {
    assert_eq!(
        words("Flight TP1351, at 07:40 — Café à Lisbon!"),
        ["flight", "tp1351", "at", "07", "40", "café", "lisbon"]
    );
    let long = "x".repeat(100);
    assert_eq!(words(&long)[0].len(), MAX_TERM);
    assert_eq!(
        counted("milk Milk bread milk"),
        [(String::from("bread"), 1), (String::from("milk"), 3)]
    );
}

#[test]
fn f16_keeps_its_values_and_rounds_to_nearest_even() {
    for (x, h) in [
        (0.0f32, 0x0000u16),
        (-0.0, 0x8000),
        (1.0, 0x3c00),
        (-2.0, 0xc000),
        (0.5, 0x3800),
        (65504.0, 0x7bff),
        (65520.0, 0x7c00),
        (f32::INFINITY, 0x7c00),
        (1.0 / 16_777_216.0, 0x0001),
        (1023.0 / 16_777_216.0, 0x03ff),
        (1.0 / 16_384.0, 0x0400),
        (1e-9, 0x0000),
    ] {
        assert_eq!(narrow(x), h, "{x}");
    }
    assert_eq!(narrow(f32::NAN) & 0x7c00, 0x7c00);
    assert!(widen(narrow(f32::NAN)).is_nan());
    // Every f16 that is a number widens and narrows back to itself.
    for h in 0..=u16::MAX {
        if h & 0x7c00 == 0x7c00 && h & 0x03ff != 0 {
            continue;
        }
        assert_eq!(narrow(widen(h)), h, "{h:#06x}");
    }
    // Halfway between 1.0 and the next f16 goes to the even one, 1.0; just
    // above it goes up.
    assert_eq!(narrow(1.0 + 1.0 / 2048.0), 0x3c00);
    assert_eq!(narrow(1.0 + 3.0 / 2048.0), 0x3c02);
    assert_eq!(narrow(1.0 + 1.0 / 2048.0 + 1.0 / 65536.0), 0x3c01);
}

#[test]
fn time_words_are_resolved_from_the_clock() {
    let today = NOW_NS - NOW_NS % DAY_NS;
    assert_eq!(resolve("today", NOW_NS), Some(today));
    assert_eq!(resolve("  Yesterday ", NOW_NS), Some(today - DAY_NS));
    assert_eq!(resolve("last week", NOW_NS), Some(NOW_NS - 7 * DAY_NS));
    assert_eq!(resolve("past month", NOW_NS), Some(NOW_NS - 30 * DAY_NS));
    assert_eq!(resolve("last 3 days", NOW_NS), Some(NOW_NS - 3 * DAY_NS));
    assert_eq!(resolve("past day", NOW_NS), Some(NOW_NS - DAY_NS));
    assert_eq!(resolve("last 1 day", NOW_NS), Some(NOW_NS - DAY_NS));
    for words in [
        "",
        "last",
        "next week",
        "last 0 days",
        "last -3 days",
        "last three days",
    ] {
        assert_eq!(resolve(words, NOW_NS), None, "{words}");
    }
    let too_many = alloc::format!("last {} days", MAX_DAYS + 1);
    assert_eq!(resolve(&too_many, NOW_NS), None);
    // Never before the epoch.
    assert_eq!(resolve("last month", DAY_NS), Some(0));
}

#[test]
fn a_vector_is_made_unit_length_and_a_bad_one_is_refused() {
    let v = unit(&[3.0, 4.0]).unwrap();
    assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
    assert_eq!(unit(&[0.0, 0.0]), None);
    assert_eq!(unit(&[f32::NAN, 1.0]), None);
    assert_eq!(unit(&[f32::INFINITY, 1.0]), None);
    assert_eq!(dot(&[1.0], &[1.0, 2.0]), 0.0);
}
