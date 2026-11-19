//! The trusted layer's asks: read from the kernel, shown on the card, and
//! answered.

use super::*;

/// How often the ask box is read: often enough that the kernel sees the
/// trusted layer alive, and a posted ask shows within a frame or two.
const ASK_EVERY_MS: u64 = 100;

/// Reads the ask box: a new ask shows as the card; one the kernel withdrew
/// goes away.
pub(super) fn read_asks(state: &mut State) {
    let now = now_ms();
    if now.saturating_sub(state.asked_ms) < ASK_EVERY_MS {
        return;
    }
    state.asked_ms = now;
    let mut buf = [0u8; 2048];
    let Ok(len) = sys::ask_read(&mut buf) else {
        return;
    };
    let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
    let mut parts = text.splitn(3, '\n');
    let posted = match (parts.next(), parts.next(), parts.next()) {
        (Some(id), Some(call), Some(question)) => id.parse().ok().map(|id| (id, call, question)),
        _ => None,
    };
    match (posted, state.card.as_ref()) {
        (Some((id, _, _)), Some(card)) if card.id == id => {}
        (Some((id, _, _)), _) if state.too_long == Some(id) => {}
        (Some((id, call, question)), _) => {
            let card = Card {
                id,
                call: call.into(),
                reason: question.into(),
                shown_ms: now,
            };
            let lines = state.painter.card_lines(state.width, &card);
            if !deskcore::trusted::fits(state.width, state.height, lines) {
                agent::refuse_too_long(state, &card);
                return;
            }
            if let Some(old) = state.card.take() {
                preview::card_gone(state, old.id);
                let effects = state.desk.hide_ask(old.id);
                deliver(state, &effects);
            }
            println!("desktop: ask {id} shows on the card");
            state.desk.set_now(now);
            let effects = state.desk.show_ask(id, lines);
            let shown = card.clone();
            state.card = Some(card);
            deliver(state, &effects);
            preview::card_shown(state, &shown);
        }
        (None, Some(card)) => {
            // Withdrawn by the kernel: it asks on the console now.
            let id = card.id;
            state.card = None;
            preview::card_gone(state, id);
            let effects = state.desk.hide_ask(id);
            deliver(state, &effects);
        }
        (None, None) => {}
    }
}

/// The card's answer goes to the kernel with the digest of the call it showed.
pub(super) fn answer_ask(state: &mut State, id: u64, yes: bool) {
    let Some(card) = state.card.take().filter(|c| c.id == id) else {
        return;
    };
    preview::card_gone(state, id);
    let digest = termcore::once::digest_call(id, &card.call);
    let word = if yes { "yes" } else { "no" };
    match sys::ask_answer(id, yes, &digest) {
        Ok(_) => println!("desktop: ask {id} answered {word}"),
        Err(error) => println!("desktop: the kernel refused ask {id}'s answer ({error:?})"),
    }
}

/// Posts `event` to the ring in the header page of `pid`'s pool `pool`; a full
/// ring drops it and counts the drop there.
pub(super) fn post(pools: &[Pool], pid: u64, pool: u32, event: &surfabi::header::Event) {
    let Some(p) = pools
        .iter()
        .find(|p| p.pid == pid && p.pool == pool && p.writable)
    else {
        return;
    };
    if (p.size as usize) < surfabi::header::PAGE {
        return;
    }
    // SAFETY: the pool is mapped read-write at `address` for `size` bytes, at
    // least a page; its first page is the header the client set up, and only
    // its ring's slots and head are written.
    let page = unsafe { &mut *(p.address as *mut [u8; surfabi::header::PAGE]) };
    surfabi::header::post(page, event);
}
