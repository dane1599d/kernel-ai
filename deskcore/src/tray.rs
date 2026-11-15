//! What the tray says (ADR 0005 §8), code's alone: the Kai chip reads the
//! panel's state, the model's warmth and the bar, never what anyone said;
//! the memory is the bar's, in 0.1 GiB steps, so it repaints only when its
//! text changes.

use crate::paint::Bar;
use crate::panel::{Panel, State};
use alloc::format;
use alloc::string::String;

/// The chip's dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    /// The model is ready: `ok`.
    Ready,
    /// The model is warming: a `dim` ring.
    Warming,
    /// No model: a `dim` dash.
    None,
}

/// The chip's dot and text. `pane_open` or `compact` shows only "Kai", so
/// the chip never repeats the pane's status row or crowds a small tray.
pub fn chip(
    panel: &Panel,
    bar: &Bar,
    now_ms: u64,
    pane_open: bool,
    compact: bool,
) -> (Dot, String) {
    let dot = match (bar.model, panel.getting_ready()) {
        (false, _) => Dot::None,
        (true, true) => Dot::Warming,
        (true, false) => Dot::Ready,
    };
    if pane_open || compact {
        return (dot, String::from("Kai"));
    }
    let text = match (dot, panel.state()) {
        (Dot::None, State::Idle) => String::from("No model"),
        (Dot::Warming, State::Idle) => String::from("Kai is getting ready"),
        (_, State::Idle) => String::from("Kai ready"),
        (_, State::Thinking { since_ms }) => {
            format!("Kai thinking {}s", now_ms.saturating_sub(*since_ms) / 1000)
        }
        (_, State::Acting { .. }) => String::from("Kai acting"),
        (_, State::Done { .. }) => String::from("Kai done"),
        (_, State::Refused { .. }) => String::from("Kai refused"),
    };
    (dot, text)
}

/// The memory in use, "2.1 GiB".
pub fn memory(bar: &Bar) -> String {
    let tenths = (bar.mem_used_mib * 10 + 512) / 1024;
    format!("{}.{} GiB", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint_tests::BAR;

    #[test]
    fn the_chip_says_the_state_and_only_kai_beside_the_pane() {
        let mut panel = Panel::new();
        assert_eq!(
            chip(&panel, &BAR, 0, false, false),
            (Dot::Ready, "Kai ready".into())
        );
        panel.set_state(State::Thinking { since_ms: 1_000 });
        assert_eq!(chip(&panel, &BAR, 5_500, false, false).1, "Kai thinking 4s");
        assert_eq!(chip(&panel, &BAR, 5_500, true, false).1, "Kai");
        assert_eq!(chip(&panel, &BAR, 5_500, false, true).1, "Kai");
        panel.set_state(State::Idle);
        panel.set_ready(false);
        assert_eq!(
            chip(&panel, &BAR, 0, false, false),
            (Dot::Warming, "Kai is getting ready".into())
        );
        let none = Bar {
            model: false,
            ..BAR
        };
        assert_eq!(
            chip(&panel, &none, 0, false, false),
            (Dot::None, "No model".into())
        );
    }

    #[test]
    fn memory_is_in_tenths_of_a_gib() {
        let at = |mib| {
            memory(&Bar {
                mem_used_mib: mib,
                ..BAR
            })
        };
        assert_eq!(at(0), "0.0 GiB");
        assert_eq!(at(2_150), "2.1 GiB");
        assert_eq!(at(2_200), "2.1 GiB");
        assert_eq!(at(2_253), "2.2 GiB");
        assert_eq!(at(1_024), "1.0 GiB");
    }
}
