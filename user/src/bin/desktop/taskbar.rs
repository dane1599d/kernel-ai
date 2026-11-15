//! The taskbar's host half: before each frame, the parts of the taskbar and
//! tray whose look changed are repainted (`deskcore::tray::damage` decides).

use super::State;

/// Damages each taskbar and tray part whose look changed since it was drawn.
pub fn damage(state: &mut State) {
    let now = super::now_ms();
    let rects = deskcore::tray::damage(&state.desk, &state.panel, &state.bar, now, &mut state.tray);
    state.extra.extend(rects);
}
