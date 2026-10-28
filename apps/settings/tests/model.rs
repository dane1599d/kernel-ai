//! The Settings model on the host: its files round trip and survive hostile
//! bytes, grants read as plain words by app, and each choice gives the intent
//! the OS carries out, or none when there is nothing to do.

use comp_settings::access::by_app;
use comp_settings::files::{Appearance, Apps, DAY_MINUTES, Updates};
use comp_settings::{Intent, Settings};

const GRANTS: &str = "kai-grants 1\n\
grant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n\
grant 42 app=browser kind=host object=example.org rights=connect from=request:7 life=session signer=- made=2026-11-02T10:15:00Z\n\
grant 43 app=notes kind=file object=/home/user/todo.txt rights=read from=install life=persistent signer=- made=2026-11-02T10:16:00Z\n";

fn settings() -> Settings {
    Settings {
        access: by_app(&kaigrants::parse(GRANTS).unwrap()),
        nappable: vec!["files".into(), "notes".into()],
        ..Settings::default()
    }
}

#[test]
fn grants_read_as_plain_words_grouped_by_app() {
    let s = settings();
    let names: Vec<&str> = s.access.iter().map(|a| a.app.as_str()).collect();
    assert_eq!(names, ["browser", "notes"]);
    let notes = &s.access[1].access;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].grant, 41);
    assert_eq!(
        notes[0].what,
        "Read, write and enumerate the folder /home/user/notes"
    );
    assert_eq!(notes[0].why, "You chose it in a file picker");
    assert_eq!(notes[1].what, "Read the file /home/user/todo.txt");
    let browser = &s.access[0].access[0];
    assert_eq!(browser.what, "Connect to example.org");
    assert_eq!(
        browser.why,
        "You allowed it when asked (log entry 7), until restart"
    );
}

#[test]
fn revoking_names_only_a_held_grant_and_its_app_goes_when_empty() {
    let mut s = settings();
    assert_eq!(s.revoke(99), None);
    let intent = s.revoke(42).unwrap();
    assert_eq!(intent, Intent::Revoke(42));
    s.applied(&intent);
    let names: Vec<&str> = s.access.iter().map(|a| a.app.as_str()).collect();
    assert_eq!(names, ["notes"]);
    assert_eq!(s.revoke(42), None);
}

#[test]
fn the_theme_button_cycles_every_look_theme() {
    let mut s = settings();
    let mut seen = Vec::new();
    for _ in 0..look::THEMES.len() {
        let intent = s.next_theme();
        s.applied(&intent);
        seen.push(s.appearance.theme.clone());
    }
    let mut want: Vec<String> = look::THEMES.iter().map(|p| p.name.into()).collect();
    want.rotate_left(1);
    assert_eq!(seen, want);
}

#[test]
fn keep_running_is_only_for_apps_that_nap_and_only_when_it_changes() {
    let mut s = settings();
    assert_eq!(s.keep_running("shell", true), None);
    assert_eq!(s.keep_running("notes", false), None);
    let on = s.keep_running("notes", true).unwrap();
    s.applied(&on);
    assert!(s.apps.keeps("notes"));
    assert_eq!(s.keep_running("notes", true), None);
}

#[test]
fn a_restart_window_must_be_two_different_minutes_of_a_day() {
    let s = settings();
    assert_eq!(s.restart_window(None), None);
    assert_eq!(s.restart_window(Some((60, 60))), None);
    assert_eq!(s.restart_window(Some((0, DAY_MINUTES))), None);
    assert_eq!(
        s.restart_window(Some((120, 300))),
        Some(Intent::RestartWindow(Some((120, 300))))
    );
}

#[test]
fn a_window_past_midnight_allows_both_sides() {
    let u = Updates {
        restart_window: Some((23 * 60, 60)),
    };
    assert!(u.allows(23 * 60 + 30) && u.allows(30));
    assert!(!u.allows(12 * 60));
    assert!(!Updates::default().allows(0));
}

#[test]
fn the_files_round_trip() {
    let a = Appearance {
        theme: look::THEMES[1].name.into(),
        legible: true,
        reduced_motion: true,
    };
    assert_eq!(Appearance::decode(&a.encode()), a);
    let mut apps = Apps::default();
    apps.set("notes", true);
    apps.set("files", true);
    assert_eq!(apps.keep_running, ["files", "notes"]);
    assert_eq!(Apps::decode(&apps.encode()), apps);
    for w in [None, Some((1, 2)), Some((1439, 0))] {
        let u = Updates { restart_window: w };
        assert_eq!(Updates::decode(&u.encode()), u);
    }
}

#[test]
fn unknown_keys_are_skipped_and_an_unknown_theme_is_the_default() {
    // [1, {"future": [1, 2], "theme": "neon", "legible": true}]
    let bytes = [
        0x82, 0x01, 0xA3, 0x66, b'f', b'u', b't', b'u', b'r', b'e', 0x82, 0x01, 0x02, 0x65, b't',
        b'h', b'e', b'm', b'e', 0x64, b'n', b'e', b'o', b'n', 0x67, b'l', b'e', b'g', b'i', b'b',
        b'l', b'e', 0xF5,
    ];
    let a = Appearance::decode(&bytes);
    assert_eq!(a.theme, Appearance::default().theme);
    assert!(a.legible);
}

#[test]
fn hostile_bytes_give_defaults_never_a_panic() {
    let samples = [
        Appearance::default().encode(),
        {
            let mut apps = Apps::default();
            apps.set("notes", true);
            apps.encode()
        },
        Updates {
            restart_window: Some((10, 20)),
        }
        .encode(),
    ];
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for sample in &samples {
        for _ in 0..3000 {
            let mut bytes = sample.clone();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed as usize) % bytes.len();
            bytes[at] = (seed >> 24) as u8;
            bytes.truncate(bytes.len() - (seed as usize >> 40) % 3);
            let _ = Appearance::decode(&bytes);
            let apps = Apps::decode(&bytes);
            assert!(apps.keep_running.windows(2).all(|w| w[0] < w[1]));
            let u = Updates::decode(&bytes);
            if let Some((s, e)) = u.restart_window {
                assert!(s < DAY_MINUTES && e < DAY_MINUTES && s != e);
            }
        }
    }
}
