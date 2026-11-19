use super::*;

const TEXT: &str = "hello from the desk\nsecond line";

fn call_for(text: &str) -> String {
    format!(
        "net_send(\"hooks.example\", \"/hook\", \"{}\")",
        digest(text)
    )
}

#[test]
fn a_send_call_reads_back_and_others_do_not() {
    let call = send_call(&call_for(TEXT)).unwrap();
    assert_eq!(call.host, "hooks.example");
    assert_eq!(call.path, "/hook");
    assert_eq!(call.digest.len(), DIGEST_HEX);
    for bad in [
        "net_send(\"hooks.example\", \"/hook\")",
        "net_send(\"hooks.example\", \"hook\", \"0123456789abcdef\")",
        "net_send(\"\", \"/hook\", \"0123456789abcdef\")",
        "net_send(\"hooks.example\", \"/hook\", \"0123456789ABCDEF\")",
        "net_send(\"hooks.example\", \"/hook\", \"0123456789abcdef\", \"x\")",
        "net_send(\"a\\\"b\", \"/hook\", \"0123456789abcdef\")",
        "net_send(\"hooks.example\", \"/a\\\\b\", \"0123456789abcdef\")",
        "fs_create(\"a\", \"/hook\", \"0123456789abcdef\")",
    ] {
        assert_eq!(send_call(bad), None, "{bad}");
    }
}

/// Every call that names `net_send` is a send's card, whether it parses or
/// not: one that does not (a quote or backslash the row should have refused)
/// is never an ordinary card (#1142's review, CRITICAL).
#[test]
fn a_call_that_names_net_send_is_a_sends_card_parsed_or_not() {
    for call in [
        call_for(TEXT),
        String::from("net_send(\"a\\\"b\", \"/hook\", \"0123456789abcdef\")"),
        String::from("net_send(\"hooks.example\", \"/a\\\\b\", \"0123456789abcdef\")"),
        String::from("net_send()"),
    ] {
        assert!(names_send(&call), "{call}");
    }
    for call in ["cpus()", "net_sender(\"a\")", "fs_create(\"net_send(\")"] {
        assert!(!names_send(call), "{call}");
    }
}

/// The digest is SHA-256's own: "abc"'s begins ba7816bf8f01cfea.
#[test]
fn the_digest_is_sha256s_first_16_hex() {
    assert_eq!(digest("abc"), "ba7816bf8f01cfea");
}

/// Only a text whose hash is the card's shows: a byte changed, added or
/// left out shows nothing; nor does a text the send row would not take.
#[test]
fn a_preview_is_made_only_for_the_cards_own_text() {
    let call = call_for(TEXT);
    let preview = SendPreview::new(7, &call, TEXT).unwrap();
    assert_eq!(
        preview.header(1, 2),
        [
            String::from("31 bytes to hooks.example/hook"),
            format!("SHA-256 {} \u{b7} lines 1-1 of 2", digest(TEXT)),
        ]
    );
    for other in [
        "hello from the desk\nsecond lin",
        "Hello from the desk\nsecond line",
        "",
    ] {
        assert_eq!(SendPreview::new(7, &call, other), None, "{other:?}");
    }
    assert_eq!(SendPreview::new(7, "cpus()", TEXT), None);
    let long = "x".repeat(MAX_TEXT + 1);
    assert_eq!(SendPreview::new(7, &call_for(&long), &long), None);
    for odd in ["caf\u{e9}", "tab\there", "bell\x07"] {
        assert_eq!(SendPreview::new(7, &call_for(odd), odd), None, "{odd:?}");
    }
}

/// Every byte shows: a space that could hide (at a line's ends, in a run),
/// and an empty line, as marks the text itself can never hold; a single
/// inner space stays a space (#1142's review).
#[test]
fn spaces_that_could_hide_and_empty_lines_show_as_marks() {
    let text = "  indented\n\n   \ntrailing  \ninner  gap\none space";
    let preview = SendPreview::new(1, &call_for(text), text).unwrap();
    assert_eq!(
        preview.shown(),
        [
            "\u{b7}\u{b7}indented",
            "\u{b6}",
            "\u{b7}\u{b7}\u{b7}",
            "trailing\u{b7}\u{b7}",
            "inner\u{b7}\u{b7}gap",
            "one space",
        ]
    );
}

/// A soft wrap starts its piece with a mark, so it never looks like one of
/// the text's own line breaks; a space at the wrap shows; every character
/// is kept.
#[test]
fn a_soft_wrap_is_marked_and_no_space_hides_at_it() {
    let measure = |t: &str| t.chars().count() as f32;
    let pieces = wrap_shown("ab cd efgh", 4.0, &measure);
    assert_eq!(pieces, ["ab c", "\u{21aa}d e", "\u{21aa}fgh"]);
    // A space at either side of a wrap shows.
    assert_eq!(
        wrap_shown("abc def", 3.0, &measure),
        ["abc", "\u{21aa}\u{b7}d", "\u{21aa}ef"]
    );
    let kept: String = pieces
        .concat()
        .replace('\u{21aa}', "")
        .replace('\u{b7}', " ");
    assert_eq!(kept, "ab cd efgh");
    // Two texts that differ only in a line break draw differently.
    let one = SendPreview::new(1, &call_for("abcdefgh"), "abcdefgh").unwrap();
    let two = SendPreview::new(1, &call_for("abcd\nefgh"), "abcd\nefgh").unwrap();
    let draw = |p: &SendPreview| -> Vec<String> {
        p.shown()
            .iter()
            .flat_map(|l| wrap_shown(l, 4.0, &measure))
            .collect()
    };
    assert_ne!(draw(&one), draw(&two));
}

/// Read through means every line has been on the screen, each in turn: a
/// jump to the end reads nothing between; scrolling back takes nothing
/// back.
#[test]
fn it_is_read_through_once_every_line_has_shown() {
    let ten = laid("a", 10);
    let mut preview = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
    assert!(!preview.on_screen(4, &ten));
    preview.scroll(9, 10, 4);
    assert!(
        !preview.on_screen(4, &ten),
        "lines 5 and 6 were never shown"
    );
    preview.scroll(-3, 10, 4);
    assert!(preview.header(4, 10)[1].ends_with("lines 4-7 of 10"));
    assert!(preview.on_screen(4, &ten));
    preview.scroll(-9, 10, 4);
    assert!(preview.on_screen(4, &ten));
    let mut short = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
    assert!(
        short.on_screen(5, &laid("a", 2)),
        "a text that fits is read at once"
    );
}

/// Laid-out lines for the read-through tests: `n` of them, told apart by
/// `layout`.
fn laid(layout: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{layout} {i}")).collect()
}

/// Another layout of as many lines starts the count again: lines seen in
/// the old one are not read in the new (#1142's re-check).
#[test]
fn a_new_layout_of_as_many_lines_starts_the_count_again() {
    let mut preview = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
    let (wide, narrow) = (laid("wide", 10), laid("narrow", 10));
    assert!(!preview.on_screen(4, &wide));
    preview.scroll(6, 10, 4);
    assert!(
        !preview.on_screen(4, &narrow),
        "lines 1-6 of the new layout unseen"
    );
    assert!(preview.seen[..6].iter().all(|seen| !seen));
    preview.scroll(-6, 10, 4);
    assert!(!preview.on_screen(4, &narrow), "lines 5 and 6 still unseen");
    preview.scroll(3, 10, 4);
    assert!(preview.on_screen(4, &narrow));
}

#[test]
fn the_preview_wire_reads_back() {
    let wire = encode_preview(12, TEXT);
    assert_eq!(parse_preview(&wire), Some((12, TEXT)));
    assert_eq!(parse_preview("panel preview x\nhi"), None);
    assert_eq!(parse_preview("panel preview 3"), None);
    let long = format!("{PREVIEW}3\n{}", "x".repeat(MAX_TEXT + 1));
    assert_eq!(parse_preview(&long), None);
}

/// Only the shell gives a send's text or asks whether its card fits; and
/// what the chat says reaches `desktop` as `panel said`, never as a preview
/// (ADR 0005 §7's tests; #1142's review).
#[test]
fn only_the_shell_posts_a_text_and_the_chat_never_does() {
    assert!(from_shell("shell"));
    for caller in ["notes-window", "comphost", "terminal-window", "desktop", ""] {
        assert!(!from_shell(caller), "{caller}");
    }
    let said = crate::panel_wire::encode_said(&encode_preview(3, TEXT));
    assert_eq!(parse_preview(&said), None);
    assert_ne!(said, encode_preview(3, TEXT));
}

#[test]
fn scrolling_stays_within_the_text() {
    let mut preview = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
    assert!(!preview.scroll(-3, 10, 4));
    assert!(preview.scroll(9, 10, 4));
    assert_eq!(preview.scrolled, 6);
    assert!(!preview.scroll(1, 10, 4));
}

/// The text and the card meet in either order; the card is read through on
/// the frame that shows its last line, once; a card that is not a send's
/// takes no text (#1142's review: attach, draw, drawn).
#[test]
fn the_slot_shows_a_text_with_its_card_and_says_when_it_is_read() {
    for text_first in [true, false] {
        let mut slot = Slot::default();
        let call = call_for(TEXT);
        let came = match text_first {
            true => {
                assert_eq!(slot.text(9, TEXT), None);
                slot.card(9, &call)
            }
            false => {
                let card = slot.card(9, &call);
                assert_eq!(card, (true, None));
                (true, slot.text(9, TEXT))
            }
        };
        assert_eq!(came, (true, Some(Came::Shown { ask: 9, bytes: 31 })));
        assert_eq!(slot.preview().map(|p| p.ask), Some(9));
        let two = laid("a", 2);
        assert_eq!(slot.drawn(1, &two), None, "one line of two");
        assert_eq!(slot.drawn(2, &two), Some(9));
        assert_eq!(slot.drawn(2, &two), None, "said once");
    }
    let mut slot = Slot::default();
    assert_eq!(slot.card(4, "cpus()"), (false, None));
    assert_eq!(slot.text(4, TEXT), None);
    assert!(slot.preview().is_none());
}

/// A text that does not match, or a send's call that does not parse, shows
/// nothing, and the card stays a send's: no Allow.
#[test]
fn the_slot_refuses_a_mismatch_and_an_unparsed_send() {
    let mut slot = Slot::default();
    slot.card(9, &call_for(TEXT));
    assert_eq!(slot.text(9, "another text"), Some(Came::Refused { ask: 9 }));
    assert!(slot.preview().is_none());
    let mut slot = Slot::default();
    let odd = "net_send(\"hooks.example\", \"/a\\\\b\", \"0123456789abcdef\")";
    assert_eq!(slot.card(9, odd), (true, None));
    assert_eq!(slot.text(9, TEXT), Some(Came::Refused { ask: 9 }));
    assert!(slot.preview().is_none());
    assert_eq!(slot.drawn(10, &laid("a", 1)), None);
}

/// A card withdrawn after the next one's text came never takes that text
/// with it (#1142's review: the withdrawal race).
#[test]
fn a_card_gone_takes_only_its_own_text() {
    let mut slot = Slot::default();
    slot.card(8, &call_for("first"));
    assert!(slot.text(8, "first").is_some());
    // The next card's text arrives before the first card's withdrawal.
    assert_eq!(slot.text(9, TEXT), None);
    assert!(slot.gone(8), "its preview went");
    assert_eq!(
        slot.card(9, &call_for(TEXT)).1,
        Some(Came::Shown { ask: 9, bytes: 31 })
    );
    assert!(!slot.gone(8), "nothing of 9's goes");
    assert_eq!(slot.preview().map(|p| p.ask), Some(9));
}
