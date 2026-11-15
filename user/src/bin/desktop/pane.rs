//! The Kai pane's host half: what a key or the send item did to the panel,
//! carried out, and the keys that scroll the pane's history.

use super::State;
use user::println;

/// HID usages the pane's history scrolls by.
pub const PAGE_UP: u16 = 0x4B;
pub const PAGE_DOWN: u16 = 0x4E;

/// What a key or the send item did to the panel, carried out: Esc's
/// interrupt of the shell's line, a newly queued request, the Apps sheet,
/// and Esc giving the focus back to the window it came from.
pub fn after_panel_key(state: &mut State, before: usize) {
    // Esc on a line the shell took: it is interrupted now, if the shell
    // still runs it.
    if state.panel.take_interrupt() {
        let line = deskcore::running::Line::Panel;
        let _ = super::interrupt_shell(state, line, "the panel's Esc");
    }
    if state.panel.waiting() > before {
        // Its length only: a panel line may hold anything typed.
        let len = match state.panel.newest() {
            Some(deskcore::panel::Request::Line(text)) => text.chars().count(),
            _ => 0,
        };
        println!("desktop: the panel queued a request ({len} characters)");
        super::wake_shell();
    }
    if state.panel.take_apps() {
        super::apps::open(state);
    }
    if state.panel.take_leave() {
        let effects = state.desk.leave_pane();
        super::deliver(state, &effects);
    }
}

/// PageUp (`up`) or PageDown: the history scrolls by its height less a line,
/// within its content, and repaints if it moved.
pub fn page(state: &mut State, up: bool) {
    let area = deskcore::paint_chat::chat_area(&state.desk);
    if area.is_empty() {
        return;
    }
    let most = (state.painter.chat_height(&state.chat, area.w) - area.h).max(0);
    let step = state.painter.history_page(area.h);
    let delta = if up { step } else { -step };
    if state.chat.scroll_by(delta, most) {
        state.extra.push(area);
    }
}
