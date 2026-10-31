//! The Settings model on the host: its files round trip and survive hostile
//! bytes, effect rows grouped by app, and each choice gives the intent
//! the OS carries out, or none when there is nothing to do.

use comp_settings::access;
use comp_settings::files::{Appearance, Apps, DAY_MINUTES, Updates};
use comp_settings::{Intent, RuleLine, RuleRef, Settings};

const GRANTS: &str = "notes\tnotes_add\tallowed\n\
browser\tnet_fetch\twithheld\n\
notes\tfs_write\twithheld\n";

fn settings() -> Settings {
    Settings {
        access: access::parse(GRANTS).unwrap(),
        nappable: vec!["files".into(), "notes".into()],
        ..Settings::default()
    }
}

#[test]
fn effect_rows_are_grouped_by_app_with_what_each_may_do() {
    let s = settings();
    let names: Vec<&str> = s.access.iter().map(|a| a.app.as_str()).collect();
    assert_eq!(names, ["browser", "notes"]);
    let notes = &s.access[1].access;
    assert_eq!(notes.len(), 2);
    assert_eq!(
        (notes[0].row.as_str(), notes[0].allowed),
        ("notes_add", true)
    );
    assert_eq!(
        (notes[1].row.as_str(), notes[1].allowed),
        ("fs_write", false)
    );
    let browser = &s.access[0].access[0];
    assert_eq!(
        (browser.row.as_str(), browser.allowed),
        ("net_fetch", false)
    );
}

#[test]
fn revoking_names_only_an_allowed_row_and_restoring_only_a_withheld_one() {
    let mut s = settings();
    // Not listed, or in the wrong state for the choice.
    assert_eq!(s.revoke("notes", "nope"), None);
    assert_eq!(s.revoke("nobody", "notes_add"), None);
    assert_eq!(s.revoke("notes", "fs_write"), None);
    assert_eq!(s.restore("notes", "notes_add"), None);
    assert_eq!(s.restore("nobody", "fs_write"), None);

    let revoke = s.revoke("notes", "notes_add").unwrap();
    assert_eq!(
        revoke,
        Intent::Revoke {
            app: "notes".into(),
            row: "notes_add".into()
        }
    );
    s.applied(&revoke);
    // The row stays listed, now withheld, and can be given back; a second
    // revoke names nothing.
    assert!(!s.access[1].access[0].allowed);
    assert_eq!(s.revoke("notes", "notes_add"), None);
    let restore = s.restore("notes", "notes_add").unwrap();
    assert_eq!(
        restore,
        Intent::Restore {
            app: "notes".into(),
            row: "notes_add".into()
        }
    );
    s.applied(&restore);
    assert!(s.access[1].access[0].allowed);
    // The other app's row of the same name is not touched.
    assert!(!s.access[0].access[0].allowed);
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

const SAVED: &str = "kai-appearance 1\ntheme = light\nscale = 150\nreduced_motion = on\n";

#[test]
fn the_shells_appearance_text_gives_the_theme_and_the_two_switches() {
    let a = Appearance::decode(SAVED.as_bytes());
    assert_eq!(a.theme, "light");
    assert!(a.reduced_motion && !a.legible);
    let a = Appearance::decode(b"kai-appearance 1\nlegible = on\n");
    assert_eq!(a.theme, Appearance::default().theme);
    assert!(a.legible);
}

#[test]
fn an_empty_or_unreadable_appearance_is_the_defaults() {
    let d = Appearance::default();
    for bad in [
        &b""[..],
        b"junk",
        b"kai-appearance 2\ntheme = light\n",
        b"kai-appearance 1\ntheme = neon\n",
        b"kai-appearance 1\nlegible = maybe\n",
        &[0xff, 0xfe, 0x00],
    ] {
        assert_eq!(Appearance::decode(bad), d, "{bad:?}");
    }
}

#[test]
fn the_shells_keep_list_is_a_header_and_one_app_a_line() {
    let apps = Apps::decode(b"kai-keep 1\nnotes\nfiles\nnotes\nBad Name\n\nterminal-window\n");
    assert_eq!(apps.keep_running, ["files", "notes", "terminal-window"]);
    assert!(apps.keeps("notes") && !apps.keeps("browser"));
    // Nothing kept is an empty answer; anything without the header is no list.
    for none in [
        &b""[..],
        b"notes\nfiles\n",
        b"kai-keep 2\nnotes\n",
        &[0xff, 0xfe],
    ] {
        assert!(Apps::decode(none).keep_running.is_empty(), "{none:?}");
    }
}

#[test]
fn the_shells_restart_window_text_gives_the_window() {
    for (text, want) in [
        ("kai-updates 1\nrestart-window 120 300\n", Some((120, 300))),
        ("kai-updates 1\nrestart-window 1439 0\n", Some((1439, 0))),
        (
            "kai-updates 1\nrestart-window 0 1\nrestart-window 5 6\n",
            Some((5, 6)),
        ),
        ("kai-updates 1\n", None),
    ] {
        assert_eq!(
            Updates::decode(text.as_bytes()).restart_window,
            want,
            "{text:?}"
        );
    }
}

#[test]
fn nothing_set_or_a_window_that_is_not_one_is_no_window() {
    for none in [
        &b""[..],
        b"junk",
        b"kai-updates 2\nrestart-window 120 300\n",
        b"restart-window 120 300\n",
        b"kai-updates 1\nrestart-window 300 300\n",
        b"kai-updates 1\nrestart-window 1440 5\n",
        b"kai-updates 1\nrestart-window 5 1440\n",
        b"kai-updates 1\nrestart-window +5 6\n",
        b"kai-updates 1\nrestart-window 5\n",
        b"kai-updates 1\nrestart-window 5 6 7\n",
        b"kai-updates 1\nrestart-window  5 6\n",
        b"kai-updates 1\nrestart-window off\n",
        &[0xff, 0xfe],
    ] {
        assert_eq!(Updates::decode(none), Updates::default(), "{none:?}");
    }
}

#[test]
fn hostile_bytes_give_defaults_never_a_panic() {
    let mut apps = Apps::default();
    apps.set("notes", true);
    let samples: [Vec<u8>; 3] = [
        SAVED.as_bytes().to_vec(),
        b"kai-keep 1\nnotes\nfiles\n".to_vec(),
        b"kai-updates 1\nrestart-window 10 20\n".to_vec(),
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
            let a = Appearance::decode(&bytes);
            assert!(look::THEMES.iter().any(|p| p.name == a.theme));
            let apps = Apps::decode(&bytes);
            assert!(apps.keep_running.windows(2).all(|w| w[0] < w[1]));
            let u = Updates::decode(&bytes);
            if let Some((s, e)) = u.restart_window {
                assert!(s < DAY_MINUTES && e < DAY_MINUTES && s != e);
            }
        }
    }
}

#[test]
fn a_rule_ref_is_the_word_rules_remove_takes() {
    assert_eq!(RuleRef::Rule(3).word(), "3");
    assert_eq!(RuleRef::Instruction(12).word(), "S12");
    for r in [RuleRef::Rule(0), RuleRef::Rule(41), RuleRef::Instruction(7)] {
        assert_eq!(RuleRef::parse(&r.word()), Some(r));
    }
    for bad in ["", "S", "s3", "S-1", "3x", "+3", "S 3", "99999999999"] {
        assert_eq!(RuleRef::parse(bad), None, "{bad}");
    }
}

#[test]
fn removing_one_kind_leaves_the_other_with_the_same_number() {
    let mut s = Settings {
        rules: vec![
            RuleLine {
                id: RuleRef::Rule(2),
                text: "a rule".into(),
            },
            RuleLine {
                id: RuleRef::Instruction(2),
                text: "an instruction".into(),
            },
        ],
        ..Settings::default()
    };
    s.applied(&Intent::RemoveRule(RuleRef::Instruction(2)));
    assert_eq!(s.rules.len(), 1);
    assert_eq!(s.rules[0].id, RuleRef::Rule(2));
}
