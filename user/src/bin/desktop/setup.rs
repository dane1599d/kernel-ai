//! What the plan line and the saved appearance set at boot, and the screen
//! waited for.

use super::*;

/// Whether the plan line has `log_bar=1`: the panel's result line is logged.
pub(super) fn logs_bar() -> bool {
    let args = user::args();
    user::arg(&args, "log_bar") == Some("1")
}

/// The panel's result line or refusal, if `shown` has one.
pub(super) fn bar_text(shown: &deskcore::panel::State) -> Option<&str> {
    match shown {
        deskcore::panel::State::Done { line } => Some(line),
        deskcore::panel::State::Refused { reason } => Some(reason),
        _ => None,
    }
}

/// Logs the panel's new result line, on a test image's plan (`log_bar=1`)
/// only, with its newlines as "; ".
pub(super) fn log_bar(enabled: bool, text: Option<&str>) {
    if let Some(text) = text.filter(|_| enabled) {
        println!("desktop: the bar says \"{}\"", text.replace('\n', "; "));
    }
}

/// Whether the plan line has `key_log=on`: the first key's code is logged once.
pub(super) fn logs_first_key() -> bool {
    let args = user::args();
    user::arg(&args, "key_log") == Some("on")
}

/// The appearance the plan line's arguments set, one `appearance_set` setting
/// each; one that is refused is left out, and said.
pub(super) fn starting_appearance() -> Appearance {
    let args = user::args();
    let mut chosen = Appearance::default();
    for setting in appearance::FILE_SETTINGS {
        let Some(value) = user::arg(&args, setting) else {
            continue;
        };
        match chosen.set(setting, value, &Shipped) {
            Ok(next) => chosen = next,
            Err(why) => println!("desktop: {setting}={value} is refused: {}", why.0),
        }
    }
    chosen
}

/// Paints in the appearance's theme from this frame on, when it changed: the
/// whole screen again, and the hardware cursor in its colours. Nothing while the
/// painter already paints in it, so a first frame in the default theme costs
/// what it did before themes (display-claim reads a key typed during it).
pub(super) fn theme(state: &mut State, screen: &Screen) {
    if state.painted_in == state.appearance {
        return;
    }
    let theme = match state.appearance.resolve(&Shipped) {
        Ok(theme) => theme,
        Err(why) => {
            println!("desktop: the appearance is refused ({}); dark it is", why.0);
            state.appearance = Appearance::default();
            return;
        }
    };
    state.painter.set_theme(&theme);
    state.extra.push(Rect::new(0, 0, state.width, state.height));
    if screen.info.cursor != 0 {
        let _ = sys::display_cursor(&arrow(state.painter.palette()), (0, 0));
    }
    println!("desktop: painting in the {} theme", theme.name);
    // Every client paints in it too, from its next poll.
    state.theme_name = String::from(theme.name.as_str());
    for client in &mut state.clients {
        client.theme(&state.theme_name);
    }
    state.painted_in = state.appearance.clone();
}

/// Waits for the screen to come back: `false` on an error.
pub(super) fn wait_for_screen() -> bool {
    let mut events = [InputEvent {
        time_ns: 0,
        kind: Kind::Screen,
        code: 0,
        value: 0,
    }; 8];
    loop {
        let Ok(n) = sys::input_read(&mut events, 1000) else {
            return false;
        };
        if events[..n]
            .iter()
            .any(|e| e.kind == Kind::Screen && e.value == 1)
        {
            return true;
        }
    }
}
