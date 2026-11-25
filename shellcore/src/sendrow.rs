//! The send row's ask and its answer (P5 close, ADR 0002 D2's `Hold`): a
//! tagged `net_send` asks on the Allow / Deny menu, and only an Allow whose
//! ONCE checks puts the send in the outbox (`sending`), where it waits
//! [`agentabi::HOLD_MS`] with `undo` live.
//!
//! The menu's line is at most [`termcore::menu::MAX_TEXT`] characters, so the
//! call it shows, and its ONCE names, carries the text's SHA-256 (its first 16
//! hex digits), as Kai's app install card carries its build's. The whole text
//! goes above the card, outside it, under a header code writes ("Kai wants to
//! send this text to <host>:"), and the card says "the text shown"
//! (on the desk, in the Kai pane beside it); a terminal
//! that cannot show it all denies the card. `show` prints it too. A send whose card cannot be shown is refused, never asked as a
//! y/n question no answer could allow.

use crate::actions::Pending;
use crate::outbox::Send;
use crate::script::Checked;
use crate::{Reply, Shell};
use agentabi::consent::Why;
use alloc::format;
use alloc::string::{String, ToString};
use termcore::once::{CallArg, call_text};

/// The value of argument `name`, if given.
fn value<'a>(values: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.as_str())
}

/// The send a `net_send` call's values make: the path `/` when none was
/// given, and one that does not start with `/` given one.
pub(crate) fn send_of(values: &[(&'static str, String)]) -> Send {
    let path = value(values, "path").unwrap_or("");
    let path = match path.starts_with('/') {
        true => path.to_string(),
        false => format!("/{path}"),
    };
    Send {
        host: value(values, "host").unwrap_or("").to_string(),
        path,
        body: value(values, "text").unwrap_or("").to_string(),
    }
}

/// The call the card shows and its ONCE names: the host, the path, and the
/// text's digest.
pub(crate) fn card_call(send: &Send) -> String {
    let digest = crate::appbuild::sha256(&send.body);
    call_text(
        agentabi::NET_SEND.name,
        &[
            CallArg::Text(&send.host),
            CallArg::Text(&send.path),
            CallArg::Text(&digest[..16]),
        ],
    )
    .unwrap_or_default()
}

/// Why `send` cannot be asked about, if it cannot: a host that is not one, or
/// a path with a space, a `#` or anything but printable ASCII.
fn unsendable(send: &Send) -> Option<&'static str> {
    if nethosts::Host::parse(&send.host).is_err() {
        return Some("that is not a host name");
    }
    // Nothing the card's call would have to escape: a quote or a backslash
    // would make the call another shape than the send row's, which `desktop`
    // reads to tie the card to its text (#1142's review).
    let plain = |b: u8| b.is_ascii_graphic() && b != b'"' && b != b'\\';
    if !send.host.bytes().all(plain) {
        return Some("that is not a host name");
    }
    if !send.path.bytes().all(|b| plain(b) && b != b'#') {
        return Some(
            "the path has a space, a #, a quote, a backslash or a character that is not plain ASCII",
        );
    }
    // The text is read where the person answers, as it is: printable ASCII
    // and line breaks only, or it is not asked about at all.
    if send.body.is_empty()
        || !send
            .body
            .bytes()
            .all(|b| b == b'\n' || (b' '..=b'~').contains(&b))
    {
        return Some("the text has characters the screen cannot show as they are");
    }
    None
}

/// The card's subject: the text is shown above the card, outside it.
const SUBJECT: &str = "Send the text shown";

/// The header code writes over the text, outside the card.
fn header(send: &Send) -> String {
    format!(
        "Kai wants to send this text ({} bytes) to {}:",
        send.body.len(),
        send.host
    )
}

/// Whether the line types `host` with a port other than 443 ("https://
/// ci.example:8443/hook"): a send goes to 443 only, and the card would not
/// show the port.
fn typed_other_port(line: &str, host: &str) -> bool {
    line.split_whitespace().any(|token| {
        let token = token
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let Some((typed, rest)) = token.split_once(':') else {
            return false;
        };
        let port: String = rest.chars().take_while(char::is_ascii_digit).collect();
        typed.eq_ignore_ascii_case(host) && !port.is_empty() && port != "443"
    })
}

impl Shell {
    /// A tagged `net_send` consent asked about: its card, Allow and Deny only,
    /// saying it waits for `undo` after the Allow. A short text is on the card
    /// itself; `show` gives any text whole.
    pub(crate) fn send_ask(
        &mut self,
        id: u64,
        request: &str,
        values: &[(&'static str, String)],
        why: Why,
    ) -> Reply {
        let mut send = send_of(values);
        if typed_other_port(request, &send.host) {
            let why = "a send goes to port 443 only";
            self.record(
                id,
                request,
                &format!("net_send to {}", send.host),
                &format!("refused: {why}"),
            );
            return Reply::err(format!("Not sent: {why}"));
        }
        if let Some(why) = unsendable(&send) {
            self.record(
                id,
                request,
                &format!("net_send to {}", send.host),
                &format!("refused: {why}"),
            );
            return Reply::err(format!("Not sent: {why}"));
        }
        if let Ok(host) = nethosts::Host::parse(&send.host) {
            send.host = host.as_str().to_string();
        }
        let call = card_call(&send);
        if call.is_empty() {
            return Reply::err("Not sent: the send cannot be shown on its card");
        }
        // The card is 120 characters: the short form of the hold line.
        let reason = Some(format!("it waits {} s for undo", agentabi::HOLD_MS / 1000));
        let checked = || Checked::shown_only(&call);
        let reply = self.ask_call_with(
            id,
            request,
            checked(),
            why,
            None,
            Some(String::from(SUBJECT)),
            reason,
        );
        let menu = self.pending.as_ref().is_some_and(|p| p.menu.is_some());
        if !menu {
            self.pending = None;
            self.record(id, request, &call, "refused: the card cannot show it");
            return Reply::err(format!(
                "Not sent: the send to {} is too long to ask about on its card",
                send.host
            ));
        }
        self.shown = Some(shown(&call, &send));
        // The whole text goes above the card, under code's header; a terminal
        // that cannot show it all denies the card (`termcore::menu::SHOWN`).
        let reply = Reply {
            text: termcore::menu::encode_shown(&header(&send), &send.body, &reply.text),
            ..reply
        };
        if let Some(pending) = &mut self.pending {
            pending.send = Some(send);
        }
        reply
    }

    /// `show` (or `answer <id> show`) for a send's card: everything it would
    /// send, the text whole, and the card again.
    pub(crate) fn send_show(&mut self, pending: crate::actions::Pending) -> Reply {
        let id = pending.id;
        let text = pending
            .send
            .as_ref()
            .map(|send| shown(&pending.checked.script, send))
            .unwrap_or_default();
        self.pending = Some(pending);
        Reply {
            ok: true,
            text,
            confirm: Some(id),
            more: false,
        }
    }

    /// The answer to a send's card: an Allow whose ONCE checks holds the send
    /// in the outbox; anything else declines it and nothing is sent.
    pub(crate) fn answer_send(&mut self, pending: Pending, text: &str) -> Reply {
        let Some(send) = pending.send.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let reply = self.hold_send(
            pending.id,
            &pending.request,
            &pending.checked.script,
            send,
            agentabi::HOLD_MS,
        );
        self.remember(&pending, &reply.text);
        reply
    }
}

/// Everything a send's `show` says: the call the card names, where it goes,
/// the hold, and the text whole.
fn shown(call: &str, send: &Send) -> String {
    format!(
        "{call}\nto {}{} (the text's SHA-256 begins {})\nAfter you allow, it waits {} s so you can undo.\ntext ({} bytes):\n{}",
        send.host,
        send.path,
        &crate::appbuild::sha256(&send.body)[..16],
        agentabi::HOLD_MS / 1000,
        send.body.len(),
        send.body
    )
}
