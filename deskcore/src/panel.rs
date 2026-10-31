//! The AI panel's line (ADR 0005 §8): a full-width `kai>` command line along
//! the bottom, which is also the launcher. Keys come from [`crate::desk::Desk`]
//! as `PanelKey` while the panel has focus; Enter queues the line for the agent
//! core (the shell), which takes it with a call to `desktop` and reports what it
//! is doing back the same way: calls go one way (ADR 0005 §7), so `desktop`
//! never calls the shell. The state shown on the right is the shell's, never
//! read from reply text.
//!
//! The line [`APPS`] is the launcher's own: it opens the Apps sheet (P4 task 3)
//! in `desktop` and never reaches the shell. What the sheet asks for comes back
//! as a line, [`Panel::send`], so it goes the way a typed one does.

use crate::desk::usage;
use crate::keymap;
use alloc::collections::VecDeque;
use alloc::string::String;
use termcore::keys::Key;
use termcore::line::{Line, Outcome};

/// The longest line the panel takes, in characters.
pub const MAX_LINE: usize = 400;
/// Requests waiting for the shell, at most: an agent that stopped taking them
/// does not make the panel grow without bound.
const MAX_WAITING: usize = 8;
/// The line that opens the Apps sheet.
pub const APPS: &str = "apps";

/// What the panel shows on its right, as the shell last reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing running: the placeholder and the model's dot.
    Idle,
    /// A request was sent and is being worked on since `since_ms`; Esc stops it.
    Thinking { since_ms: u64 },
    /// A row is running: its call in mono, and what it acts on.
    Acting { call: String, target: String },
    /// Finished: code's result line, with undo offered.
    Done { line: String },
    /// Refused or failed: the reason, under an `error` bar.
    Refused { reason: String },
}

/// What the shell takes from the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// A line typed and sent with Enter.
    Line(String),
    /// Esc while thinking: stop the request in flight.
    Stop,
}

pub struct Panel {
    line: Line,
    state: State,
    focused: bool,
    shift: u8,
    ctrl: u8,
    waiting: VecDeque<Request>,
    /// [`APPS`] was sent and the sheet has not been opened for it yet.
    apps: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            line: Line::new(MAX_LINE),
            state: State::Idle,
            focused: false,
            shift: 0,
            ctrl: 0,
            waiting: VecDeque::new(),
            apps: false,
        }
    }

    pub fn text(&self) -> &str {
        self.line.text()
    }

    /// The cursor, in characters from the start of the text.
    pub fn cursor(&self) -> usize {
        self.line.cursor()
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether the panel has the keyboard: listening, with the ember caret.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Focus came to the panel or left it. Modifiers held then are forgotten:
    /// their releases may go elsewhere.
    pub fn set_focus(&mut self, focused: bool) -> bool {
        self.shift = 0;
        self.ctrl = 0;
        let changed = self.focused != focused;
        self.focused = focused;
        changed
    }

    /// The shell's report of what it is doing.
    pub fn set_state(&mut self, state: State) -> bool {
        let changed = self.state != state;
        self.state = state;
        changed
    }

    /// A key while the panel has focus, at `now_ms`; true when the panel
    /// changed and needs drawing.
    pub fn key(&mut self, code: u16, down: bool, now_ms: u64) -> bool {
        let held = |count: &mut u8| {
            *count = match down {
                true => count.saturating_add(1),
                false => count.saturating_sub(1),
            }
        };
        match code {
            usage::LEFT_SHIFT | usage::RIGHT_SHIFT => {
                held(&mut self.shift);
                return false;
            }
            usage::LEFT_CTRL | usage::RIGHT_CTRL => {
                held(&mut self.ctrl);
                return false;
            }
            _ if !down => return false,
            _ => {}
        }
        let Some(key) = keymap::key_of(code, self.shift > 0, self.ctrl > 0) else {
            return false;
        };
        if key == Key::Esc {
            return self.escape();
        }
        match self.line.apply(key) {
            Outcome::Unchanged => false,
            Outcome::Changed => true,
            Outcome::Cleared(_) => true,
            Outcome::Submitted(text) => {
                match text.trim() {
                    "" => {}
                    APPS => self.apps = true,
                    _ => self.send(text, now_ms),
                }
                true
            }
        }
    }

    /// Queues `text` for the shell as though it was typed and sent: the Apps
    /// sheet's `install <name>`.
    pub fn send(&mut self, text: String, now_ms: u64) {
        self.queue(Request::Line(text));
        self.state = State::Thinking { since_ms: now_ms };
    }

    /// Whether [`APPS`] was sent since the last look.
    pub fn take_apps(&mut self) -> bool {
        core::mem::take(&mut self.apps)
    }

    /// Esc: stops a request being thought about, else clears the line.
    fn escape(&mut self) -> bool {
        if matches!(self.state, State::Thinking { .. }) {
            // A line still waiting never reaches the shell; one it took is
            // told to stop.
            match self.waiting.back() {
                Some(Request::Line(_)) => {
                    self.waiting.pop_back();
                }
                _ => self.queue(Request::Stop),
            }
            self.state = State::Idle;
            return true;
        }
        if self.line.text().is_empty() {
            return false;
        }
        self.line = Line::new(MAX_LINE);
        true
    }

    fn queue(&mut self, request: Request) {
        if self.waiting.len() == MAX_WAITING {
            self.waiting.pop_front();
        }
        self.waiting.push_back(request);
    }

    /// Requests the shell has not taken yet.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// The newest request waiting.
    pub fn newest(&self) -> Option<&Request> {
        self.waiting.back()
    }

    /// The oldest request the shell has not taken.
    pub fn take(&mut self) -> Option<Request> {
        self.waiting.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presses and releases each usage.
    fn tap(panel: &mut Panel, codes: &[u16]) {
        for &code in codes {
            panel.key(code, true, 1_000);
            panel.key(code, false, 1_000);
        }
    }

    #[test]
    fn a_typed_line_goes_to_the_shell_on_enter_and_the_panel_thinks() {
        let mut panel = Panel::new();
        panel.set_focus(true);
        // "Hi" with Shift held for the H, then Enter.
        panel.key(usage::LEFT_SHIFT, true, 0);
        tap(&mut panel, &[0x0B]);
        panel.key(usage::LEFT_SHIFT, false, 0);
        tap(&mut panel, &[0x0C, 0x28]);
        assert_eq!(panel.take(), Some(Request::Line("Hi".into())));
        assert_eq!(panel.take(), None);
        assert_eq!(panel.text(), "");
        assert_eq!(panel.state(), &State::Thinking { since_ms: 1_000 });
    }

    #[test]
    fn esc_stops_a_request_and_otherwise_clears_the_line() {
        let mut panel = Panel::new();
        tap(&mut panel, &[0x0B, 0x28]);
        assert!(matches!(panel.take(), Some(Request::Line(_))));
        tap(&mut panel, &[0x29]);
        assert_eq!(panel.take(), Some(Request::Stop));
        assert_eq!(panel.state(), &State::Idle);
        tap(&mut panel, &[0x0B, 0x0C]);
        tap(&mut panel, &[0x29]);
        assert_eq!(panel.text(), "");
        assert_eq!(panel.take(), None);
    }

    #[test]
    fn esc_before_the_shell_takes_a_line_takes_it_back() {
        let mut panel = Panel::new();
        tap(&mut panel, &[0x0B, 0x28]);
        tap(&mut panel, &[0x29]);
        assert_eq!(panel.take(), None);
        assert_eq!(panel.state(), &State::Idle);
    }

    #[test]
    fn a_blank_line_sends_nothing_and_the_queue_is_bounded() {
        let mut panel = Panel::new();
        tap(&mut panel, &[0x2C, 0x28]);
        assert_eq!(panel.take(), None);
        for _ in 0..20 {
            tap(&mut panel, &[0x04, 0x28]);
        }
        assert_eq!(core::iter::from_fn(|| panel.take()).count(), MAX_WAITING);
    }

    #[test]
    fn apps_opens_the_sheet_and_never_reaches_the_shell() {
        let mut panel = Panel::new();
        // "apps", then Enter.
        tap(&mut panel, &[0x04, 0x13, 0x13, 0x16, 0x28]);
        assert!(panel.take_apps());
        assert!(!panel.take_apps(), "once per Enter");
        assert_eq!(panel.take(), None);
        assert_eq!(panel.state(), &State::Idle);
        assert_eq!(panel.text(), "");
        panel.send("install notes".into(), 7);
        assert_eq!(panel.take(), Some(Request::Line("install notes".into())));
        assert_eq!(panel.state(), &State::Thinking { since_ms: 7 });
    }

    #[test]
    fn a_modifier_held_across_focus_does_not_stick() {
        let mut panel = Panel::new();
        panel.key(usage::LEFT_SHIFT, true, 0);
        panel.set_focus(true);
        tap(&mut panel, &[0x04, 0x28]);
        assert_eq!(panel.take(), Some(Request::Line("a".into())));
    }
}
