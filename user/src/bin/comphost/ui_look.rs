//! The windows in the desktop's whole appearance (ADR 0005 section 8): its
//! theme with the person's colours over it, its text size and legible type.
//! `desktop` sends the saved `kai-appearance 1` text (`surfabi`'s `Look`);
//! this resolves the theme it paints in, paints with its colours, and lays
//! every window out in its type, so rows grow with the text.

use super::*;
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use look::appearance::{Appearance, Shipped};

/// The legible type and the text size in percent that new layouts are measured
/// in: set by [`Ui::set_look`], read by [`measure`] for every instance loaded
/// after (a version loaded again starts in the type in use).
static LEGIBLE: AtomicBool = AtomicBool::new(false);
static PERCENT: AtomicU16 = AtomicU16::new(100);

/// The type styles layouts are measured in now: the default theme's, legible
/// and sized as the appearance says.
pub(super) fn type_theme() -> look::theme::Theme {
    look::theme::Theme::from_palette(&look::DARK, false).sized(
        LEGIBLE.load(Ordering::Relaxed),
        PERCENT.load(Ordering::Relaxed),
    )
}

impl Ui {
    /// Paints in the desktop's appearance `text` from now on and lays every open
    /// window out in its type: the theme with its colours over it, and its text
    /// size and legible type. Text that does not resolve, or the one in use,
    /// changes nothing. An app napped (suspended) keeps its old layout until it
    /// wakes; its frames are painted in the new colours now.
    pub(super) fn set_look<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        text: &str,
    ) {
        if text == self.look {
            return;
        }
        let resolved = Appearance::decode(text, &Shipped).and_then(|a| {
            let theme = a.resolve(&Shipped)?;
            Ok((a, theme))
        });
        let (appearance, theme) = match resolved {
            Ok(both) => both,
            Err(why) => {
                println!("comphost: the desktop's appearance is refused: {}", why.0);
                return;
            }
        };
        self.look = String::from(text);
        self.palette = theme.palette();
        self.text.set_theme(&theme);
        self.theme = theme.name.clone();
        LEGIBLE.store(appearance.legible, Ordering::Relaxed);
        PERCENT.store(appearance.text_size, Ordering::Relaxed);
        if !host.suspended() {
            let _ = serve_shown(host, |core| {
                if let Some(measure) = measure() {
                    core.set_measure(measure);
                }
                core.ui_relayout();
                Ok(())
            });
            // The laid-out frames come in, then every window is painted whole:
            // a colour change leaves no damage in a display list of tokens.
            self.pump(host);
        }
        self.redraw_all();
        println!(
            "comphost: laying out at text size {}%{} in the {} theme",
            appearance.text_size,
            if appearance.legible { " legible" } else { "" },
            theme.name
        );
    }
}
