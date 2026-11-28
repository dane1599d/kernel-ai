use super::*;

/// A `Hands` made by `Default` is the console's, as `new()` is.
#[test]
fn default_hands_are_the_consoles() {
    assert!(Hands::default().console());
    assert_eq!(Hands::default(), Hands::new());
}

/// The keys held, plain and after `E0`, until their breaks: the count the
/// menu's gate is seeded from.
#[test]
fn set1_knows_which_keys_are_held() {
    let mut keys = Set1::new();
    assert_eq!(keys.held_makes().count(), 0);
    for byte in [0x15, 0xE0, 0x4B] {
        keys.feed(byte);
    }
    let held: alloc::vec::Vec<_> = keys.held_makes().collect();
    assert_eq!(held, [(false, 0x15), (true, 0x4B)]);
    // A repeat is no second key; a break ends it.
    keys.feed(0x15);
    keys.feed(0x95);
    keys.feed(0xE0);
    keys.feed(0xCB);
    assert_eq!(keys.held_makes().count(), 0);
}

/// H2-13: each way the keys move, the console's decoder starts afresh,
/// and only then.
#[test]
fn the_keys_change_hands_each_way_the_screen_or_a_lease_moves() {
    let mut hands = Hands::new();
    assert!(hands.console());
    // The console holds the screen: no change while it does.
    assert!(!hands.settle(false, false));
    // desktop claims the screen (Ctrl+Alt+F12, or its first claim).
    assert!(hands.settle(true, false));
    assert!(!hands.console());
    assert!(!hands.settle(true, false));
    // A lease lends the keys to the console, and ends.
    assert!(hands.settle(true, true));
    assert!(hands.console());
    assert!(!hands.settle(true, true));
    assert!(hands.settle(true, false));
    // Ctrl+Alt+F12 back to the console, then to desktop again.
    assert!(hands.settle(false, false));
    assert!(hands.console());
    assert!(hands.settle(true, false));
    // The screen back to the console under a lease: the keys were the
    // console's already, so nothing changes hands.
    assert!(hands.settle(true, true));
    assert!(!hands.settle(false, false));
    // A lease flag left on while the console holds the screen counts for
    // nothing.
    assert!(!hands.settle(false, true));
}

fn feed(bytes: &[u8]) -> alloc::vec::Vec<(u16, bool)> {
    let mut set1 = Set1::new();
    bytes.iter().filter_map(|&b| set1.feed(b)).collect()
}

#[test]
fn set1_makes_and_breaks_turn_into_hid_usages() {
    // a, Enter, Esc, F12 down and up; keypad Enter and Up behind E0.
    assert_eq!(feed(&[0x1E, 0x9E]), [(0x04, true), (0x04, false)]);
    assert_eq!(
        feed(&[0x1C, 0x01, 0x58, 0xD8]),
        [(0x28, true), (0x29, true), (0x45, true), (0x45, false)]
    );
    assert_eq!(
        feed(&[0xE0, 0x1C, 0xE0, 0x48, 0xE0, 0xC8]),
        [(0x58, true), (0x52, true), (0x52, false)]
    );
    assert_eq!(feed(&[0xE0, 0x5B]), [(usage::LEFT_GUI, true)]);
    // Pause's E1 sequence and unknown codes give nothing.
    assert_eq!(feed(&[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, 0x54, 0x7F]), []);
}

#[test]
fn every_set1_usage_is_one_key() {
    let mut seen = alloc::vec::Vec::new();
    for usage in SET1.iter().chain(SET1_E0.iter().map(|(_, u)| u)) {
        if *usage != 0 {
            assert!(!seen.contains(usage), "{usage:#x} twice");
            seen.push(*usage);
        }
    }
    assert_eq!(seen.len(), 86 + 17);
}

/// Where each key of `bytes` goes, under a lease (`leased`) or not.
fn routes(bytes: &[u8], leased: bool) -> alloc::vec::Vec<(u16, bool, KeyRoute)> {
    let mut set1 = Set1::new();
    bytes
        .iter()
        .filter_map(|&b| set1.feed(b).map(|(u, d)| (u, d, set1.route(u, d, leased))))
        .collect()
}

/// A table row: what was typed, its set 1 bytes, and each key's route.
type Case<'a> = (&'a str, &'a [u8], &'a [(u16, bool, KeyRoute)]);

/// The route, key by key, for each sequence a person types under a
/// terminal's lease, and that nothing is held back without one.
#[test]
fn keys_under_a_lease_reach_desktop_only_to_switch_away() {
    use KeyRoute::*;
    let (gui, esc, ctrl, alt, tab, a) = (
        usage::LEFT_GUI,
        usage::ESCAPE,
        usage::LEFT_CTRL,
        usage::LEFT_ALT,
        usage::TAB,
        0x04,
    );
    let table: &[Case<'_>] = &[
        (
            "Super tap",
            &[0xE0, 0x5B, 0xE0, 0xDB],
            &[(gui, true, Console), (gui, false, Tap)],
        ),
        // Super+A, say Super+L: all the terminal's; desktop sees none.
        (
            "Super+letter",
            &[0xE0, 0x5B, 0x1E, 0x9E, 0xE0, 0xDB],
            &[
                (gui, true, Console),
                (a, true, Console),
                (a, false, Console),
                (gui, false, Console),
            ],
        ),
        (
            "Ctrl+Esc",
            &[0x1D, 0x01, 0x81, 0x9D],
            &[
                (ctrl, true, Both),
                (esc, true, Chord),
                (esc, false, Console),
                (ctrl, false, Both),
            ],
        ),
        // Esc down, then Ctrl, then Esc up: no chord on a release.
        (
            "Esc, Ctrl rollover",
            &[0x01, 0x1D, 0x81, 0x9D],
            &[
                (esc, true, Console),
                (ctrl, true, Both),
                (esc, false, Console),
                (ctrl, false, Both),
            ],
        ),
        (
            "Esc alone",
            &[0x01, 0x81],
            &[(esc, true, Console), (esc, false, Console)],
        ),
        (
            "a letter",
            &[0x1E, 0x9E],
            &[(a, true, Console), (a, false, Console)],
        ),
        (
            "Alt+Tab",
            &[0x38, 0x0F, 0x8F, 0xB8],
            &[
                (alt, true, Both),
                (tab, true, Chord),
                (tab, false, Console),
                (alt, false, Both),
            ],
        ),
        // Tab down, then Alt, then Tab up: no chord on a release.
        (
            "Tab, Alt rollover",
            &[0x0F, 0x38, 0x8F, 0xB8],
            &[
                (tab, true, Console),
                (alt, true, Both),
                (tab, false, Console),
                (alt, false, Both),
            ],
        ),
        // Super with a key that decodes to none (PrintScreen's E0 37,
        // Pause, a USB key mapped to `UNMAPPED`): no tap.
        (
            "Super+PrintScreen",
            &[0xE0, 0x5B, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xDB],
            &[(gui, true, Console), (gui, false, Console)],
        ),
        (
            "Super+Pause",
            &[0xE0, 0x5B, 0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, 0xE0, 0xDB],
            &[(gui, true, Console), (gui, false, Console)],
        ),
        (
            "Super+unmapped USB key",
            &[0xE0, 0x5B, UNMAPPED, UNMAPPED | 0x80, 0xE0, 0xDB],
            &[(gui, true, Console), (gui, false, Console)],
        ),
        (
            "Ctrl+Alt+F12",
            &[0x1D, 0x38, 0x58, 0xD8, 0xB8, 0x9D],
            &[
                (ctrl, true, Both),
                (alt, true, Both),
                (usage::F12, true, Screen),
                (usage::F12, false, Screen),
                (alt, false, Both),
                (ctrl, false, Both),
            ],
        ),
    ];
    for (what, bytes, want) in table {
        assert_eq!(&routes(bytes, true), want, "{what}");
        // Without a lease every key is desktop's, F12's chord aside, and
        // a Super tap is still the kernel's call.
        let free = routes(bytes, false);
        let tapped = want.iter().any(|r| r.2 == Tap);
        assert!(
            free.iter()
                .all(|r| r.2 == Desk || r.2 == Screen || (tapped && r.2 == Tap)),
            "{what}: {free:?}"
        );
    }
}

/// A pointer press between Super's press and release breaks the tap,
/// with a lease or without one.
#[test]
fn a_click_with_super_held_is_no_tap() {
    for leased in [false, true] {
        let mut set1 = Set1::new();
        assert_eq!(set1.feed(0xE0), None);
        let down = set1.feed(0x5B).unwrap();
        set1.route(down.0, down.1, leased);
        set1.break_tap();
        assert_eq!(set1.feed(0xE0), None);
        let up = set1.feed(0xDB).unwrap();
        let want = match leased {
            true => KeyRoute::Console,
            false => KeyRoute::Desk,
        };
        assert_eq!(set1.route(up.0, up.1, leased), want, "leased {leased}");
    }
}

/// Super's typematic repeat after a click is no new tap: E0 5B,
/// `break_tap`, E0 5B again, E0 DB.
#[test]
fn a_repeat_after_a_click_does_not_restart_the_tap() {
    let mut set1 = Set1::new();
    for b in [0xE0, 0x5B] {
        set1.feed(b);
    }
    set1.break_tap();
    assert_eq!(set1.feed(0xE0), None);
    let repeat = set1.feed(0x5B).unwrap();
    assert_eq!(repeat, (usage::LEFT_GUI, true));
    assert_eq!(set1.route(repeat.0, repeat.1, false), KeyRoute::Desk);
    assert_eq!(set1.feed(0xE0), None);
    let up = set1.feed(0xDB).unwrap();
    assert_eq!(set1.route(up.0, up.1, false), KeyRoute::Desk);
    // Released, the next press is a first make again: a tap.
    let taps = routes(&[0xE0, 0x5B, 0xE0, 0x5B, 0xE0, 0xDB], false);
    assert_eq!(taps.last().unwrap().2, KeyRoute::Tap, "repeats alone");
}

/// A Super whose break was lost (a USB unplug, a full ring) does not
/// stop the other Super's taps: Right Super and a letter with no break
/// for Right Super, then Left Super tapped.
#[test]
fn a_lost_break_of_one_super_leaves_the_others_taps() {
    let routes = routes(&[0xE0, 0x5C, 0x1E, 0x9E, 0xE0, 0x5B, 0xE0, 0xDB], false);
    assert_eq!(
        routes.last(),
        Some(&(usage::LEFT_GUI, false, KeyRoute::Tap)),
        "{routes:?}"
    );
}

/// Without a lease a lone Super tap is still [`KeyRoute::Tap`], and
/// Super with PrintScreen, Pause or an unmapped USB key is no tap:
/// `desktop` never sees those keys, so only this side can tell.
#[test]
fn without_a_lease_only_a_lone_super_is_a_tap() {
    use KeyRoute::*;
    let gui = usage::LEFT_GUI;
    assert_eq!(
        routes(&[0xE0, 0x5B, 0xE0, 0xDB], false),
        [(gui, true, Desk), (gui, false, Tap)]
    );
    for (what, bytes) in [
        (
            "PrintScreen",
            &[0xE0, 0x5B, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xDB][..],
        ),
        (
            "Pause",
            &[0xE0, 0x5B, 0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, 0xE0, 0xDB],
        ),
        (
            "unmapped",
            &[0xE0, 0x5B, UNMAPPED, UNMAPPED | 0x80, 0xE0, 0xDB],
        ),
        ("letter", &[0xE0, 0x5B, 0x1E, 0x9E, 0xE0, 0xDB]),
    ] {
        let free = routes(bytes, false);
        assert!(free.iter().all(|r| r.2 == Desk), "Super+{what}: {free:?}");
    }
}

#[test]
fn ctrl_alt_is_tracked_across_left_and_right() {
    let mut set1 = Set1::new();
    for b in [0x1D, 0xE0, 0x38] {
        set1.feed(b);
    }
    assert!(set1.ctrl_alt());
    set1.feed(0x9D);
    assert!(!set1.ctrl_alt());
    set1.feed(0xE0);
    set1.feed(0x1D);
    assert!(set1.ctrl_alt());
}

/// Feeds `bytes` and returns whether the last key was a new Ctrl+Alt+F12
/// press, as the kernel decides it.
fn pressed(keys: &mut Set1, bytes: &[u8]) -> bool {
    let mut out = false;
    for &b in bytes {
        if let Some((code, down)) = keys.feed(b) {
            out = keys.route(code, down, false) == KeyRoute::Screen && keys.switch_press(down);
        }
    }
    out
}

const CTRL: u8 = 0x1D;
const ALT: u8 = 0x38;
const F12: u8 = 0x58;

/// A held Ctrl+Alt+F12 switches once: its typematic repeats do not, and
/// the next press, after the break, does.
#[test]
fn a_held_switch_chord_switches_once() {
    let mut keys = Set1::new();
    assert!(!pressed(&mut keys, &[CTRL, ALT]));
    assert!(pressed(&mut keys, &[F12]));
    assert!(!pressed(&mut keys, &[F12]));
    assert!(!pressed(&mut keys, &[F12]));
    assert!(!pressed(&mut keys, &[F12 | 0x80]));
    assert!(pressed(&mut keys, &[F12]));
}

/// Ctrl let go before F12: the break still ends the press.
#[test]
fn ctrl_let_go_first_does_not_hold_the_press() {
    let mut keys = Set1::new();
    pressed(&mut keys, &[CTRL, ALT]);
    assert!(pressed(&mut keys, &[F12]));
    pressed(&mut keys, &[CTRL | 0x80]);
    assert!(!pressed(&mut keys, &[F12 | 0x80]));
    assert!(!pressed(&mut keys, &[CTRL]));
    assert!(pressed(&mut keys, &[F12]));
}

/// F12 let go after the switch with the chord still held, then pressed again.
#[test]
fn f12_let_go_after_the_switch_switches_again() {
    let mut keys = Set1::new();
    pressed(&mut keys, &[CTRL, ALT]);
    assert!(pressed(&mut keys, &[F12]));
    assert!(!pressed(&mut keys, &[F12 | 0x80]));
    assert!(pressed(&mut keys, &[F12]));
}

/// A lost F12 break: the next press is taken for a repeat until another
/// key is touched, and then it switches. Plain F12 never sets the press.
#[test]
fn a_lost_break_is_forgiven_by_the_next_other_key() {
    let mut keys = Set1::new();
    pressed(&mut keys, &[CTRL, ALT]);
    assert!(pressed(&mut keys, &[F12]));
    // The break never arrives; Alt is let go and pressed again.
    pressed(&mut keys, &[ALT | 0x80, ALT]);
    assert!(pressed(&mut keys, &[F12]));
    let mut plain = Set1::new();
    assert!(!pressed(&mut plain, &[F12]));
    pressed(&mut plain, &[CTRL, ALT]);
    assert!(pressed(&mut plain, &[F12]));
}

/// Another key's release while F12 is held is not a new press: Shift let go
/// mid-hold leaves the typematic repeats repeats.
#[test]
fn another_keys_release_does_not_end_the_press() {
    let mut keys = Set1::new();
    pressed(&mut keys, &[0x2A, CTRL, ALT]);
    assert!(pressed(&mut keys, &[F12]));
    assert!(!pressed(&mut keys, &[0x2A | 0x80]));
    assert!(!pressed(&mut keys, &[F12]));
}
