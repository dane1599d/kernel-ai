//! A send answered on the desk (ADR 0005 §7, "Amended 2026-11-19"): the
//! trusted card shows the send row's call, `net_send("host", "/path",
//! "<16 hex>")`, whose last argument is the first 16 hex digits of the text's
//! SHA-256; the Kai pane shows the text itself, outside the card, as a send
//! preview.
//!
//! The shell carries the text to `desktop` (`panel preview <id>`), but a text
//! is shown only when `desktop`'s own hash of it begins with the digest in
//! the call the kernel posted for that ask ([`SendPreview::new`]); one that
//! does not match is never drawn, and its card is never allowed. The card
//! takes no Allow until the preview has been drawn (`Desk::needs_preview`,
//! `Desk::preview_drawn`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The send row's action, as its call names it.
pub const ACTION: &str = "net_send";
/// How many hex digits of the text's SHA-256 the call carries.
pub const DIGEST_HEX: usize = 16;
/// The longest text a send carries (`agentabi`'s `SEND_MAX`).
pub const MAX_TEXT: usize = 4096;
/// The request that carries a send's text to `desktop`: `panel preview
/// <id>`, a newline, then the text.
pub const PREVIEW: &str = "panel preview ";
/// The shell asks before it offers a send's card on the desk: `yes` where
/// the card and the open pane fit side by side, `no` (the console asks).
pub const FITS: &str = "panel send-fits";
/// The lines a send's card takes: its title, the call, the reason.
pub const CARD_LINES: i32 = 4;

/// What a send's call on the card says: where it goes, and which text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendCall {
    pub host: String,
    pub path: String,
    /// The first [`DIGEST_HEX`] hex digits of the text's SHA-256, lower case.
    pub digest: String,
}

/// The send `call` names, if it is exactly the send row's call:
/// `net_send("<host>", "<path>", "<16 lower-case hex>")`, its host and path
/// plain ASCII with no quote or backslash (the row allows none). `None` for
/// any other call.
pub fn send_call(call: &str) -> Option<SendCall> {
    let args = call
        .strip_prefix(ACTION)?
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let mut parts = args.split(", ");
    let mut quoted = || {
        let part = parts.next()?;
        let inner = part.strip_prefix('"')?.strip_suffix('"')?;
        let plain = inner
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\');
        plain.then(|| String::from(inner))
    };
    let (host, path, digest) = (quoted()?, quoted()?, quoted()?);
    let hex = digest.len() == DIGEST_HEX
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if parts.next().is_some() || host.is_empty() || !path.starts_with('/') || !hex {
        return None;
    }
    Some(SendCall { host, path, digest })
}

/// The first [`DIGEST_HEX`] hex digits of `text`'s SHA-256, lower case.
pub fn digest(text: &str) -> String {
    let mut sha = termcore::once::Sha256::new();
    sha.update(text.as_bytes());
    let bytes = sha.finish();
    let mut hex = String::with_capacity(DIGEST_HEX);
    for byte in &bytes[..DIGEST_HEX / 2] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A send's text, shown in the Kai pane while its card is up: only one
/// whose digest is the card's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendPreview {
    /// The ask its card answers.
    pub ask: u64,
    pub call: SendCall,
    pub text: String,
    /// Lines scrolled past at the top.
    pub scrolled: usize,
    /// The furthest line that has been on the screen, counted from 1.
    pub read_to: usize,
}

impl SendPreview {
    /// The preview of `text` for ask `ask`, whose card shows `call` as the
    /// kernel posted it: `None` unless `call` is a send's and `text`, at most
    /// [`MAX_TEXT`] bytes, hashes to its digest.
    pub fn new(ask: u64, call: &str, text: &str) -> Option<Self> {
        let call = send_call(call)?;
        (text.len() <= MAX_TEXT && digest(text) == call.digest).then(|| Self {
            ask,
            call,
            text: String::from(text),
            scrolled: 0,
            read_to: 0,
        })
    }

    /// The header code writes over the text: where it goes and how much,
    /// and which of its `total` lines show now, `rows` at a time.
    pub fn header(&self, rows: usize, total: usize) -> String {
        let first = (self.scrolled + 1).min(total);
        let last = (self.scrolled + rows).min(total);
        format!(
            "{} bytes to {}{} \u{b7} SHA-256 {} \u{b7} lines {first}-{last} of {total}",
            self.text.len(),
            self.call.host,
            self.call.path,
            self.call.digest,
        )
    }

    /// The text as it is shown: its own line breaks kept, and anything
    /// outside printable ASCII as `\u{...}` (`askcard::shown`), every
    /// character kept. The painter wraps each line to the width.
    pub fn shown(&self) -> String {
        self.text
            .split('\n')
            .map(crate::askcard::shown)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Scrolls by `lines` (down for a positive count), within `total` lines
    /// of which `rows` fit: true when the view moved.
    pub fn scroll(&mut self, lines: i32, total: usize, rows: usize) -> bool {
        let most = total.saturating_sub(rows);
        let to = (self.scrolled as i64 + i64::from(lines)).clamp(0, most as i64) as usize;
        let moved = to != self.scrolled;
        self.scrolled = to;
        moved
    }

    /// The text's lines from its scroll, `rows` of `total`, are on the
    /// screen now: whether the whole text has been, the last line included.
    pub fn on_screen(&mut self, rows: usize, total: usize) -> bool {
        self.read_to = self.read_to.max((self.scrolled + rows).min(total));
        self.read_to >= total
    }
}

/// A send's text as its request to `desktop`.
pub fn encode_preview(ask: u64, text: &str) -> String {
    format!("{PREVIEW}{ask}\n{text}")
}

/// A [`encode_preview`] request read back: the ask and the text. `None` for
/// anything else, or a text longer than [`MAX_TEXT`].
pub fn parse_preview(request: &str) -> Option<(u64, &str)> {
    let (head, text) = request.strip_prefix(PREVIEW)?.split_once('\n')?;
    let digits = !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit());
    let ask = head.parse().ok().filter(|_| digits)?;
    (text.len() <= MAX_TEXT).then_some((ask, text))
}

#[cfg(test)]
mod tests {
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
            "fs_create(\"a\", \"/hook\", \"0123456789abcdef\")",
        ] {
            assert_eq!(send_call(bad), None, "{bad}");
        }
    }

    /// The digest is SHA-256's own: "abc"'s begins ba7816bf8f01cfea.
    #[test]
    fn the_digest_is_sha256s_first_16_hex() {
        assert_eq!(digest("abc"), "ba7816bf8f01cfea");
    }

    /// Only a text whose hash is the card's shows: a byte changed, added or
    /// left out shows nothing.
    #[test]
    fn a_preview_is_made_only_for_the_cards_own_text() {
        let call = call_for(TEXT);
        let preview = SendPreview::new(7, &call, TEXT).unwrap();
        assert_eq!(
            preview.header(1, 2),
            alloc::format!(
                "31 bytes to hooks.example/hook \u{b7} SHA-256 {} \u{b7} lines 1-1 of 2",
                digest(TEXT)
            )
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
    }

    #[test]
    fn its_text_keeps_every_character_and_escapes_what_is_not_plain() {
        let text = "caf\u{e9}\n\ttab";
        let preview = SendPreview::new(1, &call_for(text), text).unwrap();
        assert_eq!(preview.shown(), "caf\\u{e9}\n\\u{9}tab");
    }

    /// Read through means the last line has been on the screen, at once or
    /// by scrolling; scrolling back does not take it back.
    #[test]
    fn it_is_read_through_once_its_last_line_has_shown() {
        let mut preview = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
        assert!(!preview.on_screen(4, 10));
        preview.scroll(3, 10, 4);
        assert!(!preview.on_screen(4, 10));
        assert_eq!(
            preview.header(4, 10).rsplit(" \u{b7} ").next(),
            Some("lines 4-7 of 10")
        );
        preview.scroll(9, 10, 4);
        assert!(preview.on_screen(4, 10));
        preview.scroll(-9, 10, 4);
        assert!(preview.on_screen(4, 10));
        let mut short = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
        assert!(short.on_screen(5, 2), "a text that fits is read at once");
    }

    #[test]
    fn the_preview_wire_reads_back() {
        let wire = encode_preview(12, TEXT);
        assert_eq!(parse_preview(&wire), Some((12, TEXT)));
        assert_eq!(parse_preview("panel preview x\nhi"), None);
        assert_eq!(parse_preview("panel preview 3"), None);
        let long = alloc::format!("{PREVIEW}3\n{}", "x".repeat(MAX_TEXT + 1));
        assert_eq!(parse_preview(&long), None);
    }

    #[test]
    fn scrolling_stays_within_the_text() {
        let mut preview = SendPreview::new(1, &call_for(TEXT), TEXT).unwrap();
        assert!(!preview.scroll(-3, 10, 4));
        assert!(preview.scroll(9, 10, 4));
        assert_eq!(preview.scrolled, 6);
        assert!(!preview.scroll(1, 10, 4));
    }
}
