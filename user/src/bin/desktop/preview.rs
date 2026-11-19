//! A send's text beside its card (ADR 0005 §7, "Amended 2026-11-19"): the
//! shell hands the text (`panel preview <id>`), the kernel posts the card's
//! call, and `deskcore::sendview::Slot` shows the text only once its hash is
//! the card's digest. Any card whose call names `net_send` is a send's,
//! parsed or not, and takes no Allow until its whole text has been on the
//! screen (`Desk::preview_drawn`); PageUp, PageDown and the wheel scroll it
//! while the card is up (`Effect::PreviewScroll`).

use super::*;
use deskcore::sendview::{self, Came};

/// The shell's `panel preview <id>` and `panel send-fits` requests (the
/// caller is checked as the shell's before any `panel` request comes here).
/// `None` for another request.
pub(super) fn preview_request(state: &mut State, text: &str) -> Option<String> {
    if text == sendview::FITS {
        let fits = state.desk.card_beside_pane(sendview::CARD_LINES);
        return Some(String::from(if fits { "yes" } else { "no" }));
    }
    let (ask, body) = sendview::parse_preview(text)?;
    let came = state.sends.text(ask, body);
    came_to(state, came);
    Some(String::from("ok"))
}

/// The card `card` just went up: a send's waits for its text, and the pane
/// opens for it.
pub(super) fn card_shown(state: &mut State, card: &Card) {
    let (send, came) = state.sends.card(card.id, &card.call);
    if !send {
        return;
    }
    println!(
        "desktop: ask {} is a send's: it waits for its text",
        card.id
    );
    let effects = state.desk.needs_preview();
    deliver(state, &effects);
    came_to(state, came);
}

/// Card `id` went down: its text goes with it, and nothing of another's.
pub(super) fn card_gone(state: &mut State, id: u64) {
    if state.sends.gone(id) {
        state.extra.push(state.desk.pane_area());
    }
}

fn came_to(state: &mut State, came: Option<Came>) {
    match came {
        Some(Came::Shown { ask, bytes }) => {
            println!("desktop: ask {ask} shows its text in the pane ({bytes} bytes)");
            state.extra.push(state.desk.pane_area());
        }
        Some(Came::Refused { ask }) => println!(
            "desktop: ask {ask}'s text does not match its card: not shown, and its card cannot be allowed"
        ),
        None => {}
    }
}

/// After a frame went to the screen: the lines it showed count as read, and
/// once the last one has been on the screen the card may be allowed.
pub(super) fn drawn(state: &mut State, at_ms: u64) {
    if state.desk.pane().is_none() {
        return;
    }
    let area = paint_chat::chat_area(&state.desk);
    let Some(preview) = state.sends.preview() else {
        return;
    };
    let laid = state.painter.preview_layout(area, preview);
    if let Some(ask) = state.sends.drawn(laid.rows, &laid.lines) {
        state.desk.preview_drawn(ask, at_ms);
        println!("desktop: ask {ask}'s whole text has been on the screen");
    }
}

/// PageUp, PageDown (`pages`) or the wheel (`detents`) while a send's card
/// is up: the text scrolls, a page less a line or three lines a detent.
pub(super) fn scroll(state: &mut State, pages: i32, detents: i32) {
    let area = paint_chat::chat_area(&state.desk);
    let Some(preview) = state.sends.preview() else {
        return;
    };
    let laid = state.painter.preview_layout(area, preview);
    let page = laid.rows.saturating_sub(1).max(1) as i32;
    let by = pages.saturating_mul(page) - detents.saturating_mul(paint_chat::WHEEL_LINES);
    let Some(preview) = state.sends.preview_mut() else {
        return;
    };
    if preview.scroll(by, laid.lines.len(), laid.rows) {
        state.extra.push(area);
        println!("desktop: the send's text scrolled");
    }
}
