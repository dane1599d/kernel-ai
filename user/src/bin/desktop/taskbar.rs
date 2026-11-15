//! The tray's host half: before each frame, what the tray would show now is
//! compared with what it showed, and only a part whose text changed is
//! repainted: the chip on a state, warmth or pane change ("Kai thinking 4s"
//! counts each second), the memory in 0.1 GiB steps, the clock once a
//! minute. A still desk repaints none of it.

use super::State;
use alloc::string::String;
use deskcore::clock;
use deskcore::tray::{self, Dot};

/// What the tray showed last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    chip: Option<(Dot, String, bool)>,
    memory: Option<String>,
    minute: Option<Option<u64>>,
}

/// Damages each tray part whose text changed since it was last drawn.
pub fn damage(state: &mut State) {
    let l = state.desk.layout();
    let t = l.tray;
    let compact = t.memory.is_none();
    let open = state.desk.pane().is_some();
    let now = super::now_ms();
    let (dot, text) = tray::chip(&state.panel, &state.bar, now, open, compact);
    let chip = Some((dot, text, state.desk.unread()));
    if chip != state.tray.chip {
        state.tray.chip = chip;
        state.extra.push(t.chip);
    }
    let memory = Some(tray::memory(&state.bar));
    if let Some(r) = t.memory
        && memory != state.tray.memory
    {
        state.tray.memory = memory;
        state.extra.push(r);
    }
    let minute = Some(state.bar.utc_s.map(clock::minute));
    if minute != state.tray.minute {
        state.tray.minute = minute;
        state.extra.push(t.clock);
    }
}
