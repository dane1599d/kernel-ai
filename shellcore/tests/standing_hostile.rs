//! Hostile standing instructions (P3 task 11): a rule line as the user might
//! type it and a `kai-standing 1` store as it comes back from disk, cut, mutated
//! or bent at every byte. Each is read or refused, never a panic, and what reads
//! is canonical: it describes or writes itself back the same.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use shellcore::standing::{Standing, grammar};

const LINE: &str = "on minimize(notes) -> app_suspend(notes) until restore(notes)";

const STORE: &str = "kai-standing 1\nS 1 on=minimize:notes do=app_suspend:notes until=restore:notes paused=0\nS 2 on=idle:15 do=app_suspend:mail until=- paused=1\nS 4 on=time:1320 do=app_suspend:game until=time:450 paused=0\n";

fn read_line(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| grammar::parse(text));
    if let Ok(draft) = result {
        let again = grammar::parse(&draft.describe()).expect("a rule that read describes itself");
        assert_eq!(again, draft);
    }
    assert!(peak <= 16 * 1024, "{peak} bytes for {} bytes", text.len());
}

fn read_store(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| Standing::parse(text));
    if let Some(store) = result {
        assert_eq!(store.text(), text);
    }
    assert!(
        peak <= 64 * 1024 + 16 * text.len(),
        "{peak} bytes for {} bytes",
        text.len()
    );
}

#[test]
fn the_samples_read() {
    assert!(grammar::parse(LINE).is_ok());
    assert_eq!(Standing::parse(STORE).map(|s| s.rules().len()), Some(3));
}

#[test]
fn mutated_rule_lines_are_read_or_refused() {
    hostile::fuzz(LINE.as_bytes(), 8, 30_000, read_line);
    hostile::truncations(LINE.as_bytes(), |_, cut| read_line(cut));
}

#[test]
fn mutated_stores_are_read_or_refused() {
    hostile::fuzz(STORE.as_bytes(), 8, 30_000, read_store);
    hostile::truncations(STORE.as_bytes(), |_, cut| read_store(cut));
}
