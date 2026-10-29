//! `view`: the lines a screen shows, in a form it can read back exactly. It
//! changes nothing, not even the cursor, and the journal skips it, so a screen
//! can ask as often as it scrolls.
//!
//! ```text
//! ok view <component> <label> <path> <lines> <cursor> <flags>
//! <n>:<text>        a line, exactly as the file holds it
//! <n>~<text>        a line cut or cleaned for the screen: edit it with `c`, not here
//! ```
//!
//! `flags` is `-` or letters: `c` the file differs from the opened version, `u`
//! there is an edit to undo, `b` the text is built. The reply holds fewer lines
//! than asked when the next would not fit one message; the screen asks again
//! from the line after the last it got.

use crate::session::Session;
use alloc::format;
use alloc::string::String;

/// Most lines one `view` answers.
pub const MAX_VIEW: usize = 64;
/// Most characters of a line it carries.
pub const MAX_VIEW_CHARS: usize = 200;
/// Bytes of lines it stops at, so the reply fits one message with its header.
const MAX_BODY: usize = 6 * 1024;

/// A line as `view` carries it, and whether that is the line exactly.
fn carried(text: &str) -> (String, bool) {
    let mut exact = true;
    let mut out = String::new();
    for (count, c) in text.chars().enumerate() {
        if count == MAX_VIEW_CHARS {
            exact = false;
            break;
        }
        if c.is_control() && c != '\t' {
            exact = false;
            out.push('?');
        } else {
            out.push(c);
        }
    }
    (out, exact)
}

/// The reply body for lines `from` to `to` of the current file.
pub fn render(session: &Session, from: usize, to: usize) -> String {
    let file = session.current();
    let index = session.current_index();
    let mut flags = String::new();
    if session.file_changed(index) {
        flags.push('c');
    }
    if session.can_undo() {
        flags.push('u');
    }
    if session.built.is_some() {
        flags.push('b');
    }
    if flags.is_empty() {
        flags.push('-');
    }
    let mut out = format!(
        "view {} {} {} {} {} {flags}",
        session.component,
        session.label,
        file.path,
        file.len(),
        session.line()
    );
    let to = to.min(file.len()).min(from.saturating_add(MAX_VIEW - 1));
    let mut body = 0;
    for n in from.max(1)..=to {
        let Some(text) = file.line(n) else { break };
        let (text, exact) = carried(text);
        body += text.len() + 8;
        if body > MAX_BODY {
            break;
        }
        out.push_str(&format!("\n{n}{}{text}", if exact { ':' } else { '~' }));
    }
    out
}
