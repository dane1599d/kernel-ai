use crate::keymap::Sym::{Char as C, Dead as D, None as N};
use crate::keymap::{Accent, LAYOUTS, Layout, Sym, char_of, compose};
use std::collections::BTreeSet;
use std::vec::Vec;

/// The main block every layout names: letters, digits, the symbol keys and
/// the ISO key left of Z.
fn main_block() -> Vec<u16> {
    let mut keys: Vec<u16> = (0x04..=0x27).chain(0x2D..=0x38).collect();
    keys.push(0x64);
    keys
}

fn layout(id: &str) -> &'static Layout {
    Layout::by_id(id).unwrap()
}

fn typed(id: &str, usage: u16, shift: bool, altgr: bool) -> Sym {
    layout(id).sym(usage, shift, altgr, false)
}

#[test]
fn every_layout_names_each_main_block_key_once_and_has_its_own_id() {
    let ids: BTreeSet<&str> = LAYOUTS.iter().map(|l| l.id).collect();
    assert_eq!(ids.len(), LAYOUTS.len(), "ids are unique");
    assert_eq!(LAYOUTS[0].id, "us", "US comes first");
    for layout in LAYOUTS {
        let usages: Vec<u16> = layout.rows.iter().map(|r| r.0).collect();
        let unique: BTreeSet<u16> = usages.iter().copied().collect();
        assert_eq!(
            unique.len(),
            usages.len(),
            "{}: a key named twice",
            layout.id
        );
        assert_eq!(
            unique,
            main_block().into_iter().collect(),
            "{}: the main block, no more and no less",
            layout.id
        );
        assert!(!layout.name.is_empty());
    }
    assert!(Layout::by_id("xx").is_none());
}

#[test]
fn every_latin_letter_is_on_one_plain_key_in_every_layout() {
    for layout in LAYOUTS {
        for letter in 'a'..='z' {
            let keys = layout.rows.iter().filter(|r| r.1 == C(letter)).count();
            assert_eq!(keys, 1, "{}: {letter} on {keys} keys", layout.id);
        }
    }
}

#[test]
fn the_us_table_types_what_char_of_types() {
    let us = layout("us");
    for usage in 0..=0xFF {
        for (shift, caps) in [(false, false), (true, false), (false, true), (true, true)] {
            let want = char_of(usage, shift, caps).map_or(N, C);
            assert_eq!(
                us.sym(usage, shift, false, caps),
                want,
                "usage {usage:#x} shift {shift} caps {caps}"
            );
        }
    }
    assert!(us.rows.iter().all(|r| r.3 == N), "US has no AltGr level");
}

#[test]
fn shared_keys_space_and_the_keypad_type_alike_but_for_the_decimal_key() {
    for layout in LAYOUTS {
        assert_eq!(layout.sym(0x2C, true, false, false), C(' '));
        assert_eq!(layout.sym(0x59, false, false, false), C('1'));
        assert_eq!(layout.sym(0x57, false, false, false), C('+'));
        assert_eq!(
            layout.sym(0x28, false, false, false),
            N,
            "Enter types nothing here"
        );
    }
    assert_eq!(layout("de").sym(0x63, false, false, false), C(','));
    assert_eq!(layout("nordic").sym(0x63, false, false, false), C(','));
    assert_eq!(layout("gb").sym(0x63, false, false, false), C('.'));
}

#[test]
fn uk_moves_at_and_quote_and_has_the_pound() {
    assert_eq!(typed("gb", 0x20, true, false), C('£'), "Shift 3");
    assert_eq!(typed("gb", 0x1F, true, false), C('"'), "Shift 2");
    assert_eq!(typed("gb", 0x34, true, false), C('@'));
    assert_eq!(typed("gb", 0x32, false, false), C('#'));
    assert_eq!(typed("gb", 0x35, true, false), C('¬'));
    assert_eq!(typed("gb", 0x21, false, true), C('€'), "AltGr 4");
    assert_eq!(typed("gb", 0x08, false, true), C('é'));
}

#[test]
fn german_swaps_y_and_z_and_has_umlauts_and_dead_accents() {
    assert_eq!(typed("de", 0x1C, false, false), C('z'), "the US Y key");
    assert_eq!(typed("de", 0x1D, true, false), C('Y'), "the US Z key");
    assert_eq!(typed("de", 0x2D, false, false), C('ß'));
    assert_eq!(typed("de", 0x2F, false, false), C('ü'));
    assert_eq!(typed("de", 0x33, true, false), C('Ö'));
    assert_eq!(typed("de", 0x34, false, false), C('ä'));
    assert_eq!(typed("de", 0x2E, false, false), D(Accent::Acute));
    assert_eq!(typed("de", 0x35, false, false), D(Accent::Circumflex));
    assert_eq!(typed("de", 0x35, true, false), C('°'));
    assert_eq!(typed("de", 0x14, false, true), C('@'), "AltGr Q");
    assert_eq!(typed("de", 0x08, false, true), C('€'), "AltGr E");
    assert_eq!(typed("de", 0x25, false, true), C('['), "AltGr 8");
    assert_eq!(typed("de", 0x64, false, true), C('|'));
    assert_eq!(typed("de", 0x24, true, false), C('/'), "Shift 7");
}

#[test]
fn french_is_azerty_with_the_digits_on_shift() {
    assert_eq!(typed("fr", 0x04, false, false), C('q'), "the US A key");
    assert_eq!(typed("fr", 0x14, false, false), C('a'), "the US Q key");
    assert_eq!(typed("fr", 0x1A, false, false), C('z'), "the US W key");
    assert_eq!(typed("fr", 0x1D, false, false), C('w'), "the US Z key");
    assert_eq!(typed("fr", 0x33, false, false), C('m'), "M is right of L");
    assert_eq!(typed("fr", 0x10, false, false), C(','), "the US M key");
    assert_eq!(typed("fr", 0x1F, false, false), C('é'));
    assert_eq!(typed("fr", 0x1F, true, false), C('2'));
    assert_eq!(typed("fr", 0x26, false, false), C('ç'));
    assert_eq!(typed("fr", 0x27, false, true), C('@'), "AltGr 0");
    assert_eq!(typed("fr", 0x2F, false, false), D(Accent::Circumflex));
    assert_eq!(typed("fr", 0x2F, true, false), D(Accent::Diaeresis));
    assert_eq!(typed("fr", 0x35, true, false), N, "² has no Shift");
}

#[test]
fn spanish_has_enye_cedilla_and_the_inverted_marks() {
    assert_eq!(typed("es", 0x33, false, false), C('ñ'));
    assert_eq!(typed("es", 0x33, true, false), C('Ñ'));
    assert_eq!(typed("es", 0x32, false, false), C('ç'));
    assert_eq!(typed("es", 0x2E, false, false), C('¡'));
    assert_eq!(typed("es", 0x2E, true, false), C('¿'));
    assert_eq!(typed("es", 0x34, false, false), D(Accent::Acute));
    assert_eq!(typed("es", 0x34, true, false), D(Accent::Diaeresis));
    assert_eq!(typed("es", 0x1F, false, true), C('@'), "AltGr 2");
    assert_eq!(typed("es", 0x20, true, false), C('·'), "Shift 3");
}

#[test]
fn nordic_has_the_three_vowels_and_altgr_brackets() {
    assert_eq!(typed("nordic", 0x2F, false, false), C('å'));
    assert_eq!(typed("nordic", 0x34, true, false), C('Ä'));
    assert_eq!(typed("nordic", 0x33, false, false), C('ö'));
    assert_eq!(typed("nordic", 0x1F, false, true), C('@'), "AltGr 2");
    assert_eq!(typed("nordic", 0x21, true, false), C('¤'), "Shift 4");
    assert_eq!(typed("nordic", 0x25, false, true), C('['), "AltGr 8");
    assert_eq!(typed("nordic", 0x30, false, false), D(Accent::Diaeresis));
    assert_eq!(typed("nordic", 0x30, false, true), D(Accent::Tilde));
    assert_eq!(typed("nordic", 0x35, true, false), C('½'));
}

#[test]
fn caps_lock_turns_only_keys_whose_shift_is_their_capital() {
    let caps = |id: &str, usage, shift| layout(id).sym(usage, shift, false, true);
    assert_eq!(caps("de", 0x2F, false), C('Ü'));
    assert_eq!(caps("de", 0x2F, true), C('ü'), "Shift undoes Caps Lock");
    assert_eq!(caps("es", 0x33, false), C('Ñ'));
    assert_eq!(caps("nordic", 0x2F, false), C('Å'));
    assert_eq!(caps("fr", 0x14, false), C('A'));
    assert_eq!(caps("fr", 0x1F, false), C('é'), "Shift gives 2, not É");
    assert_eq!(
        caps("de", 0x2D, false),
        C('ß'),
        "ß has no one-letter capital"
    );
    assert_eq!(caps("de", 0x1E, false), C('1'));
    assert_eq!(
        layout("de").sym(0x08, false, true, true),
        C('€'),
        "AltGr ignores Caps Lock"
    );
}

#[test]
fn dead_keys_compose_with_vowels_space_and_themselves() {
    use Accent::*;
    assert_eq!(compose(Acute, C('e')), Some('é'));
    assert_eq!(compose(Acute, C('E')), Some('É'));
    assert_eq!(compose(Grave, C('a')), Some('à'));
    assert_eq!(compose(Circumflex, C('o')), Some('ô'));
    assert_eq!(compose(Tilde, C('n')), Some('ñ'));
    assert_eq!(compose(Diaeresis, C('u')), Some('ü'));
    assert_eq!(compose(Diaeresis, C('Y')), Some('Ÿ'));
    assert_eq!(compose(Acute, C(' ')), Some('´'));
    assert_eq!(compose(Circumflex, D(Circumflex)), Some('^'));
    assert_eq!(compose(Acute, C('x')), None, "no x with an acute");
    assert_eq!(compose(Acute, D(Grave)), None);
    assert_eq!(compose(Tilde, N), None);
    for accent in [Grave, Acute, Circumflex, Tilde, Diaeresis] {
        assert_eq!(compose(accent, C(' ')), Some(accent.spacing()));
    }
}

#[test]
fn every_dead_key_in_a_layout_composes_its_own_spacing_character() {
    for layout in LAYOUTS {
        for &(usage, plain, shift, alt) in layout.rows {
            for sym in [plain, shift, alt] {
                if let D(accent) = sym {
                    assert_eq!(
                        compose(accent, sym),
                        Some(accent.spacing()),
                        "{} {usage:#x}",
                        layout.id
                    );
                }
            }
        }
    }
}
