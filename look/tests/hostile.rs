//! Hostile `kai-theme 1` and `kai-appearance 1` text (ADR 0005 §8, ADR 0005's
//! parser-hardening row): every shipped theme and a saved appearance, mutated,
//! cut at every byte and swept field by field. Each parses or is refused,
//! never panics, and what parses passes every check.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use look::appearance::{Appearance, Shipped};
use look::theme::{self, Theme};

fn read_theme(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(theme) = Theme::parse(text) {
        theme.check().unwrap();
        assert_eq!(Theme::parse(&theme.encode()), Ok(theme));
    }
}

fn read_appearance(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(appearance) = Appearance::decode(text, &Shipped) {
        appearance.resolve(&Shipped).unwrap();
    }
}

#[test]
fn hostile_themes_parse_or_are_refused() {
    for (seed, theme) in theme::shipped().iter().enumerate() {
        let sample = theme.encode();
        hostile::fuzz(sample.as_bytes(), seed as u64 + 1, 3_000, read_theme);
        hostile::truncations(sample.as_bytes(), |_, cut| read_theme(cut));
    }
}

#[test]
fn hostile_appearances_decode_or_are_refused() {
    let saved = Appearance::default()
        .set("theme", "light", &Shipped)
        .and_then(|a| a.set("token", "accent #1F5FBF", &Shipped))
        .and_then(|a| a.set("text_size", "120", &Shipped))
        .unwrap()
        .encode();
    hostile::fuzz(saved.as_bytes(), 99, 3_000, read_appearance);
    hostile::truncations(saved.as_bytes(), |_, cut| read_appearance(cut));
}

#[test]
fn oversized_text_is_refused_before_it_is_read() {
    let long = format!("{}\n{}", theme::MAGIC, "; x\n".repeat(theme::MAX_BYTES));
    assert!(Theme::parse(&long).is_err());
}
