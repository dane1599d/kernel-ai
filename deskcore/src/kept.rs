//! What `desktop` keeps across a lost screen (H3-25). The console takes the
//! screen on Ctrl+Alt+F12 and for an ask it must answer, and `desktop`
//! builds its desk, connections, pools and frame again when the screen comes
//! back. The person's Kai pane (the line typed, the shell's state, the
//! history) and the queues between the apps and the shell are no part of the
//! screen: the shell keeps its side of each while `desktop`'s pid is the
//! same (`user/src/bin/shell/waiting.rs`), so `desktop` keeps its side too.
//! The ask card and a send's text are drawn state and start again: the card
//! comes back from the kernel's ask box. The sends the shell holds, and the
//! done line's undo, start again as well, until the shell next reports them,
//! as before: nothing of consent is carried across a lost screen.

use crate::chat::Chat;
use crate::panel::Panel;
use crate::{effects, files, net_wire, running, settings_wire};

#[derive(Default)]
pub struct Kept {
    pub panel: Panel,
    pub chat: Chat,
    pub effects: effects::Queue,
    pub running: running::Running,
    pub files: files::Files,
    pub settings: settings_wire::Queue,
    pub net: net_wire::Queue,
    pub models: settings_wire::Queue,
}

impl Kept {
    /// Ready for the next claim's desk, which has no focus and starts with
    /// Caps Lock off: the panel loses its focus and the keys it held (the
    /// Ctrl and Alt of Ctrl+Alt+F12 come up on the console), and its Caps
    /// Lock goes off with the desk's, so the two stay in step. The held
    /// sends and the done line's undo go, as they did when the whole panel
    /// was made anew: the shell reports the sends again.
    pub fn for_claim(mut self) -> Self {
        self.panel.set_focus(false);
        self.panel.set_caps(false);
        self.panel.set_sends(alloc::vec::Vec::new());
        self.panel.entry = None;
        self.panel.send_state = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Who;
    use crate::running::Line;

    /// The pane and the queues come back with the screen as they were; the
    /// focus and Caps Lock do not, as the new desk has neither, and the held
    /// sends wait for the shell's next report, as before.
    #[test]
    fn the_pane_and_the_queues_outlive_a_lost_screen() {
        let mut kept = Kept::default();
        kept.panel.set_focus(true);
        // `a` and `b` typed in the pane, not sent.
        for code in [0x04, 0x05] {
            kept.panel.key(code, true, 0);
            kept.panel.key(code, false, 0);
        }
        assert!(kept.chat.push(Who::You, "what time is it"));
        kept.running.took_panel(true);
        kept.effects.put("notes-window", "effect 3 AAAA");
        kept.panel.set_caps(true);
        kept.panel.set_sends(alloc::vec![crate::sends::HeldSend {
            entry: 9,
            host: "example.com".into(),
            due_ms: 30_000,
            sending: false,
        }]);
        let mut kept = kept.for_claim();
        assert!(!kept.panel.focused());
        assert_eq!(kept.panel.text(), "ab");
        assert!(kept.panel.sends().is_empty());
        // Caps Lock went off with the desk's: `c` types lower case.
        kept.panel.set_focus(true);
        kept.panel.key(0x06, true, 0);
        kept.panel.key(0x06, false, 0);
        assert_eq!(kept.panel.text(), "abc");
        assert!(!kept.chat.is_empty());
        assert!(kept.running.is(Line::Panel));
        let taken = kept
            .effects
            .take_app(4096, "notes-window")
            .expect("the app's line still waits");
        assert_eq!(taken.lines, ["effect 3 AAAA"]);
    }
}
