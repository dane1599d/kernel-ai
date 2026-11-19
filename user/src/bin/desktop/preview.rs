//! A send's text beside its card (ADR 0005 §7, "Amended 2026-11-19"): the
//! shell hands the text (`panel preview <id>`), the kernel posts the card's
//! call, and the pane shows the text only once `deskcore::sendview` finds
//! its hash to be the card's digest. The card takes no Allow until the whole
//! text has been on the screen (`Desk::preview_drawn`); PageUp, PageDown and
//! the wheel scroll it while the card is up (`Effect::PreviewScroll`).

use super::*;
use deskcore::sendview::{self, SendPreview};

/// The shell's `panel preview <id>` and `panel send-fits` requests. `None`
/// for another request.
pub(super) fn preview_request(state: &mut State, text: &str) -> Option<String> {
    if text == sendview::FITS {
        let fits = state.desk.card_beside_pane(sendview::CARD_LINES);
        return Some(String::from(if fits { "yes" } else { "no" }));
    }
    let (ask, body) = sendview::parse_preview(text)?;
    state.preview_text = Some((ask, String::from(body)));
    attach(state);
    Some(String::from("ok"))
}

/// The card `card` just went up: a send's waits for its text, and the pane
/// opens for it.
pub(super) fn card_shown(state: &mut State, card: &Card) {
    state.preview = None;
    if sendview::send_call(&card.call).is_none() {
        return;
    }
    println!(
        "desktop: ask {} is a send's: it waits for its text",
        card.id
    );
    let effects = state.desk.needs_preview();
    deliver(state, &effects);
    attach(state);
}

/// The card went down: its text goes with it.
pub(super) fn card_gone(state: &mut State) {
    if state.preview.take().is_some() {
        state.extra.push(state.desk.pane_area());
    }
    state.preview_text = None;
}

/// The text and the card, once both are here: shown only if the text's hash
/// is the card's digest. A text that does not match is never drawn, and its
/// card stays closed to Allow.
fn attach(state: &mut State) {
    let (Some(card), Some((ask, text))) = (state.card.as_ref(), state.preview_text.as_ref()) else {
        return;
    };
    if card.id != *ask || state.preview.is_some() {
        return;
    }
    match SendPreview::new(card.id, &card.call, text) {
        Some(preview) => {
            println!(
                "desktop: ask {} shows its text in the pane ({} bytes)",
                card.id,
                preview.text.len()
            );
            state.preview = Some(preview);
            state.preview_unseen = true;
            state.extra.push(state.desk.pane_area());
        }
        None => println!(
            "desktop: ask {}'s text does not match its card: not shown, and its card cannot be allowed",
            card.id
        ),
    }
}

/// After a frame went to the screen: the lines it showed count as read, and
/// once the last one has been on the screen the card may be allowed.
pub(super) fn drawn(state: &mut State, at_ms: u64) {
    if !state.preview_unseen || state.desk.pane().is_none() {
        return;
    }
    let area = paint_chat::chat_area(&state.desk);
    let Some(preview) = state.preview.as_mut() else {
        return;
    };
    let (lines, rows) = state.painter.preview_layout(area, preview);
    if !preview.on_screen(rows, lines.len()) {
        return;
    }
    let ask = preview.ask;
    state.preview_unseen = false;
    state.desk.preview_drawn(ask, at_ms);
    println!("desktop: ask {ask}'s whole text has been on the screen");
}

/// PageUp, PageDown (`pages`) or the wheel (`detents`) while a send's card
/// is up: the text scrolls, a page less a line or three lines a detent.
pub(super) fn scroll(state: &mut State, pages: i32, detents: i32) {
    let area = paint_chat::chat_area(&state.desk);
    let Some(preview) = state.preview.as_mut() else {
        return;
    };
    let (lines, rows) = state.painter.preview_layout(area, preview);
    let page = rows.saturating_sub(1).max(1) as i32;
    let by = pages.saturating_mul(page) - detents.saturating_mul(paint_chat::WHEEL_LINES);
    if preview.scroll(by, lines.len(), rows) {
        state.extra.push(area);
        println!("desktop: the send's text scrolled");
    }
}
