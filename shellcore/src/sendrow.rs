//! The send row's ask and its answer (P5 close, ADR 0002 D2's `Hold`): a
//! tagged `net_send` asks on the Allow / Deny menu, and only an Allow whose
//! ONCE checks puts the send in the outbox (`sending`), where it waits
//! [`agentabi::HOLD_MS`] with `undo` live.
//!
//! The menu's line is at most [`termcore::menu::MAX_TEXT`] characters, so the
//! call it shows, and its ONCE names, carries the text's SHA-256 (its first 16
//! hex digits), as Kai's app install card carries its build's; `show` prints
//! the text. A send whose card cannot be shown is refused, never asked as a
//! y/n question no answer could allow.

use crate::actions::Pending;
use crate::outbox::Send;
use crate::script::Checked;
use crate::{Reply, Shell};
use agentabi::consent::Why;
use alloc::format;
use alloc::string::{String, ToString};
use termcore::once::{CallArg, call_text};

/// How much of the text `show` prints with the card.
const SHOWN_TEXT: usize = 200;

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

impl Shell {
    /// A tagged `net_send` consent asked about: its card, Allow and Deny only,
    /// saying it waits for `undo` after the Allow.
    pub(crate) fn send_ask(
        &mut self,
        id: u64,
        request: &str,
        values: &[(&'static str, String)],
        why: Why,
    ) -> Reply {
        let send = send_of(values);
        let call = card_call(&send);
        if call.is_empty() {
            return Reply::err("Not sent: the send cannot be shown on its card");
        }
        let reason = crate::agentcore::hold_reason(&agentabi::NET_SEND);
        let reply = self.ask_call_with(
            id,
            request,
            Checked::shown_only(&call),
            why,
            None,
            None,
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
        let shown: String = send.body.chars().take(SHOWN_TEXT).collect();
        let more = match send.body.chars().count() > SHOWN_TEXT {
            true => "…",
            false => "",
        };
        if let Some(text) = &mut self.shown {
            text.push_str(&format!("\nto {}{}: {shown}{more}", send.host, send.path));
        }
        if let Some(pending) = &mut self.pending {
            pending.send = Some(send);
        }
        reply
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
