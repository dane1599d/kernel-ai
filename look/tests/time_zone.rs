//! The `time_zone` setting (ADR 0005 §8): the tray clock's fixed offset, kept
//! in the saved `kai-appearance 1` file with the rest.

use look::appearance::{Appearance, FILE_SETTINGS, SETTINGS, Shipped, ZONE_MAX, ZONE_MIN};

fn zone(value: &str) -> Result<i16, String> {
    Appearance::default()
        .set("time_zone", value, &Shipped)
        .map(|a| a.time_zone_min)
        .map_err(|why| why.0)
}

#[test]
fn offsets_are_read_in_hours_and_minutes() {
    for (text, minutes) in [
        ("UTC", 0),
        ("+0", 0),
        ("+2", 120),
        ("+02:00", 120),
        ("-9:30", -570),
        ("+05:45", 345),
        ("+14:00", ZONE_MAX),
        ("-12:00", ZONE_MIN),
    ] {
        assert_eq!(zone(text), Ok(minutes), "{text}");
    }
}

#[test]
fn what_is_not_a_fixed_offset_is_refused() {
    for text in [
        "",
        "2",
        "UTC+2",
        "utc",
        "+",
        "+:30",
        "+2:",
        "+2:5",
        "+2:10",
        "+2:60",
        "+15:00",
        "-13:00",
        "+14:15",
        "+002",
        "+1:000",
        "+٣",
        "+2 ",
        " +2",
        "Europe/Paris",
        "+2h",
        "--2",
    ] {
        assert!(zone(text).is_err(), "{text:?} was taken");
    }
}

#[test]
fn a_saved_zone_comes_back_and_a_default_file_is_unchanged() {
    let plain = Appearance::default();
    assert!(!plain.encode().contains("time_zone"));
    assert_eq!(
        plain.encode(),
        "kai-appearance 1\ntheme = dark\nscale = 100\ntext_size = 100\nreduced_motion = off\nlegible = off\n"
    );
    for minutes in [-570, -60, 120, 345, ZONE_MAX] {
        let set = plain.set("time_zone", &look::appearance::zone_text(minutes), &Shipped);
        let saved = set.unwrap().encode();
        let read = Appearance::decode(&saved, &Shipped).unwrap();
        assert_eq!(read.time_zone_min, minutes, "{saved}");
        assert_eq!(read.encode(), saved);
    }
}

#[test]
fn the_file_takes_it_once_and_the_agents_list_does_not_name_it() {
    let text = "kai-appearance 1\ntheme = dark\ntime_zone = +02:00\n";
    assert_eq!(
        Appearance::decode(text, &Shipped).unwrap().time_zone_min,
        120
    );
    let twice = "kai-appearance 1\ntime_zone = +02:00\ntime_zone = UTC\n";
    assert!(Appearance::decode(twice, &Shipped).is_err());
    assert!(Appearance::decode("kai-appearance 1\ntime_zone = x\n", &Shipped).is_err());
    assert_eq!(&FILE_SETTINGS[..SETTINGS.len()], &SETTINGS);
    assert_eq!(FILE_SETTINGS.len(), SETTINGS.len() + 1);
}

#[test]
fn a_change_says_which_zone_it_made() {
    let before = Appearance::default();
    let after = before.set("time_zone", "-05:00", &Shipped).unwrap();
    assert_eq!(after.change(&before), "time zone -05:00 (was UTC)");
}
