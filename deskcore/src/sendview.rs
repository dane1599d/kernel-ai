//! A send answered on the desk (ADR 0005 §7, "Amended 2026-11-19"): the
//! trusted card shows the send row's call, `net_send("host", "/path",
//! "<16 hex>")`, whose last argument is the first 16 hex digits of the text's
//! SHA-256; the Kai pane shows the text itself, outside the card, as a send
//! preview.
//!
//! The shell carries the text to `desktop` (`panel preview <id>`; only the
//! shell may, [`from_shell`]), but a text is shown only when `desktop`'s own
//! hash of it begins with the digest in the call the kernel posted for that
//! ask ([`SendPreview::new`]). Any card whose call names `net_send` is a
//! send's card ([`names_send`]), parsed or not: one whose call does not
//! parse, or whose text does not match, never shows a text and never takes
//! an Allow. The card takes no Allow until the whole text has been on the
//! screen (`Desk::needs_preview`, `Desk::preview_drawn`); [`Slot`] keeps
//! which text goes with which card.

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
/// What the preview shows for a space that could hide (at a line's start or
/// end, in a run of two or more, at a wrap), for an empty line, and at the
/// start of a line the pane wrapped (a soft wrap, not one of the text's own
/// line breaks): the text is printable ASCII and line breaks only
/// ([`SendPreview::new`]), so no mark can be one of its own bytes.
pub const SPACE_MARK: char = '\u{b7}';
pub const EMPTY_MARK: char = '\u{b6}';
pub const WRAP_MARK: char = '\u{21aa}';

/// Whether `caller` may give the panel a send's text, or ask whether a
/// send's card fits: the shell alone, as with every `panel` request. Not an
/// app, and not the chat, which reaches `desktop` only as `panel said` text.
pub fn from_shell(caller: &str) -> bool {
    caller == "shell"
}

/// Whether `call` names the send row: its card is a send's, whatever else
/// it says. A send's card takes no Allow without its text read through.
pub fn names_send(call: &str) -> bool {
    termcore::menu::names_send(call)
}

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
/// any other call, and for a send's call written any other way.
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
    /// Which of the laid-out lines have been on the screen, each one: the
    /// whole text is read only when every line has been, not only the last.
    pub seen: Vec<bool>,
    /// The laid-out lines `seen` counts: another layout, even one of as
    /// many lines, starts the count again.
    pub layout: Vec<String>,
}

impl SendPreview {
    /// The preview of `text` for ask `ask`, whose card shows `call` as the
    /// kernel posted it: `None` unless `call` is a send's, `text` is at most
    /// [`MAX_TEXT`] bytes of printable ASCII and line breaks (the send row's
    /// own rule), and it hashes to the call's digest.
    pub fn new(ask: u64, call: &str, text: &str) -> Option<Self> {
        let call = send_call(call)?;
        let plain = text
            .bytes()
            .all(|b| b == b'\n' || (b' '..=b'~').contains(&b));
        (plain && text.len() <= MAX_TEXT && digest(text) == call.digest).then(|| Self {
            ask,
            call,
            text: String::from(text),
            scrolled: 0,
            seen: Vec::new(),
            layout: Vec::new(),
        })
    }

    /// The header code writes over the text, as two lines: where it goes
    /// and how much; then the digest and which of its `total` lines show
    /// now, `rows` at a time.
    pub fn header(&self, rows: usize, total: usize) -> [String; 2] {
        let first = (self.scrolled + 1).min(total);
        let last = (self.scrolled + rows).min(total);
        [
            format!(
                "{} bytes to {}{}",
                self.text.len(),
                self.call.host,
                self.call.path
            ),
            format!(
                "SHA-256 {} \u{b7} lines {first}-{last} of {total}",
                self.call.digest
            ),
        ]
    }

    /// The text's lines as they are shown, every byte of it visible: a space
    /// that could hide (at a line's start or end, or in a run of two or
    /// more) as [`SPACE_MARK`], an empty line as [`EMPTY_MARK`]. The painter
    /// wraps each by character ([`wrap_shown`]), keeping all.
    pub fn shown(&self) -> Vec<String> {
        self.text
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    return String::from(EMPTY_MARK);
                }
                let chars: Vec<char> = line.chars().collect();
                let last = chars.len() - 1;
                (0..chars.len())
                    .map(|i| {
                        let space = chars[i] == ' ';
                        let edge = i == 0 || i == last;
                        let run =
                            (i > 0 && chars[i - 1] == ' ') || (i < last && chars[i + 1] == ' ');
                        match space && (edge || run) {
                            true => SPACE_MARK,
                            false => chars[i],
                        }
                    })
                    .collect()
            })
            .collect()
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

    /// The text laid out as `lines`, `rows` of them from its scroll, is on
    /// the screen now: whether every one of its lines has been, each in
    /// turn, not only its last (a jump to the end reads nothing in between).
    /// A new layout (another width or size), even of as many lines, starts
    /// the count again (#1142's re-check).
    pub fn on_screen(&mut self, rows: usize, lines: &[String]) -> bool {
        let total = lines.len();
        if self.layout != lines {
            self.seen = alloc::vec![false; total];
            self.layout = lines.to_vec();
        }
        let end = (self.scrolled + rows).min(total);
        for seen in &mut self.seen[self.scrolled.min(end)..end] {
            *seen = true;
        }
        self.seen.iter().all(|&seen| seen)
    }
}

/// What a step of [`Slot`] came to, for `desktop`'s log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Came {
    /// Card `ask` shows its text now.
    Shown { ask: u64, bytes: usize },
    /// Card `ask`'s text does not match its call, or its call does not
    /// parse: nothing is shown, and the card takes no Allow.
    Refused { ask: u64 },
}

/// Which send's text goes with which card, in `desktop`: the card up now,
/// the text the shell gave, and the preview made of the two.
#[derive(Clone, Debug, Default)]
pub struct Slot {
    /// The card up now: its ask, its call, and whether it is a send's.
    card: Option<(u64, String, bool)>,
    /// The text the shell gave, for its ask.
    text: Option<(u64, String)>,
    preview: Option<SendPreview>,
    /// The preview has not yet been read through on the screen.
    unread: bool,
}

impl Slot {
    /// Card `ask` with `call` went up: whether it is a send's card, which
    /// takes no Allow until its text is read through.
    pub fn card(&mut self, ask: u64, call: &str) -> (bool, Option<Came>) {
        let send = names_send(call);
        self.preview = None;
        self.unread = false;
        self.card = Some((ask, String::from(call), send));
        let came = if send { self.attach() } else { None };
        (send, came)
    }

    /// The shell gave the text for ask `ask`.
    pub fn text(&mut self, ask: u64, text: &str) -> Option<Came> {
        self.text = Some((ask, String::from(text)));
        self.attach()
    }

    /// Card `ask` went down: its card, its text and its preview go, nothing
    /// that is another card's. True when a preview went off the screen.
    pub fn gone(&mut self, ask: u64) -> bool {
        if self.card.as_ref().is_some_and(|(id, ..)| *id == ask) {
            self.card = None;
        }
        if self.text.as_ref().is_some_and(|(id, _)| *id == ask) {
            self.text = None;
        }
        let shown = self.preview.as_ref().is_some_and(|p| p.ask == ask);
        if shown {
            self.preview = None;
            self.unread = false;
        }
        shown
    }

    /// The text and the card, once both are here: shown only if the card is
    /// a send's whose call parses and the text's hash is its digest.
    fn attach(&mut self) -> Option<Came> {
        let (ask, call, send) = self.card.as_ref()?;
        let (for_ask, text) = self.text.as_ref()?;
        if !send || for_ask != ask || self.preview.is_some() {
            return None;
        }
        match SendPreview::new(*ask, call, text) {
            Some(preview) => {
                let bytes = preview.text.len();
                self.preview = Some(preview);
                self.unread = true;
                Some(Came::Shown { ask: *ask, bytes })
            }
            None => Some(Came::Refused { ask: *ask }),
        }
    }

    /// The text shown now, if any.
    pub fn preview(&self) -> Option<&SendPreview> {
        self.preview.as_ref()
    }

    pub fn preview_mut(&mut self) -> Option<&mut SendPreview> {
        self.preview.as_mut()
    }

    /// A frame showed `rows` of the text's laid-out `lines` from its
    /// scroll: the card's ask once the whole text has now been read through,
    /// the first time only.
    pub fn drawn(&mut self, rows: usize, lines: &[String]) -> Option<u64> {
        if !self.unread {
            return None;
        }
        let preview = self.preview.as_mut()?;
        if !preview.on_screen(rows, lines) {
            return None;
        }
        self.unread = false;
        Some(preview.ask)
    }
}

/// `line` (one of [`SendPreview::shown`]'s) cut into pieces no wider than
/// `width` by `measure`, character by character, every character kept: a
/// piece the pane wrapped onto starts with [`WRAP_MARK`], and a space at
/// either side of a wrap shows as [`SPACE_MARK`], so a soft wrap never looks
/// like one of the text's own line breaks and no space hides at it.
pub fn wrap_shown(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    for ch in line.chars() {
        piece.push(ch);
        let alone = piece.chars().count() == 1 || piece == alloc::format!("{WRAP_MARK}{ch}");
        if !alone && measure(&piece) > width {
            piece.pop();
            pieces.push(core::mem::take(&mut piece));
            piece.push(WRAP_MARK);
            piece.push(ch);
        }
    }
    pieces.push(piece);
    let count = pieces.len();
    for (i, piece) in pieces.iter_mut().enumerate() {
        if i + 1 < count && piece.ends_with(' ') {
            piece.pop();
            piece.push(SPACE_MARK);
        }
        if i > 0 {
            *piece = piece.replacen(
                &alloc::format!("{WRAP_MARK} "),
                &alloc::format!("{WRAP_MARK}{SPACE_MARK}"),
                1,
            );
        }
    }
    pieces
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
#[path = "sendview_tests.rs"]
mod tests;
