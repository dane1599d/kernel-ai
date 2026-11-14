//! The trusted layer's rules (ADR 0005 §7): the ask card takes the keyboard
//! and only y, n or a click on its buttons answers it; a key or click by the
//! user in the agent's window takes over from the agent.
//!
//! A card on the desktop answers nothing until it has been on screen, and the
//! keyboard and buttons quiet, for [`termcore::menu::SETTLE_MS`]: every key or
//! button pressed before then starts that quiet time again, judged by the
//! event's own time ([`Desk::set_now`]), so someone typing straight through
//! the card's arrival never answers it. A key held since before it never
//! does. And no card here came from a line typed at the trusted terminal (a
//! desktop line, an app's ask, the agent's call), so `y` takes two steps: the
//! first arms the card, which then says so; any other key disarms it and
//! starts the quiet time again; a second fresh `y` runs it. A click on Run or
//! Don't, after the quiet time, answers at once; so does `n` (#864, the
//! manager's reviews).

use super::{Button, Desk, Effect, Focus, Hit, Input, WindowId, usage};
use crate::geom::Point;
use crate::trusted::{self, Ask};
use alloc::vec::Vec;

impl Desk {
    /// Shows the ask card for log entry `id`, `lines` of text tall, above
    /// everything. It takes the keyboard; keys held now are dropped when they
    /// come up, and `desktop` drops the input the kernel has queued before it.
    pub fn show_ask(&mut self, id: u64, lines: i32) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some((old, _)) = self.ask {
            self.damage
                .add(trusted::card(self.width, self.height, &old));
        }
        let back = match self.ask {
            Some((_, back)) => back,
            None => self.focus,
        };
        let ask = Ask {
            id,
            lines,
            armed: false,
        };
        self.ask = Some((ask, back));
        // Open only once drawn ([`Desk::card_drawn`]).
        self.ask_open_ms = u64::MAX;
        self.ask_drawn = None;
        self.ask_pressed_ms = 0;
        self.ask_entered_run = false;
        self.held = self.down.clone();
        self.grab = None;
        self.damage
            .add(trusted::card(self.width, self.height, &ask));
        self.set_focus(Focus::Ask(id), &mut effects);
        effects
    }

    /// Takes the card down without an answer: its two minutes ran out, or the
    /// request ended. Focus goes back where it was.
    pub fn hide_ask(&mut self, id: u64) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.ask.is_some_and(|(ask, _)| ask.id == id) {
            self.end_ask(&mut effects);
        }
        effects
    }

    /// `desktop` put the card up now on the screen at `at_ms`: its quiet time
    /// starts then, or at the last press if that came later.
    pub fn card_drawn(&mut self, at_ms: u64) {
        if self.ask.is_none() || self.ask_drawn.is_some() {
            return;
        }
        self.ask_drawn = Some(at_ms);
        self.ask_open_ms = at_ms
            .max(self.ask_pressed_ms)
            .saturating_add(termcore::menu::SETTLE_MS);
    }

    /// A key or button pressed at `now_ms` while the card is up: before the
    /// card opened, it starts the quiet time again.
    /// A press stamped before the card was drawn (queued while `desktop` was
    /// busy) never counts and never moves the open time; the open time only
    /// ever moves later.
    fn pressed(&mut self) -> bool {
        let at = self.now_ms;
        self.ask_pressed_ms = self.ask_pressed_ms.max(at);
        let Some(drawn) = self.ask_drawn else {
            return false;
        };
        if at < drawn {
            return false;
        }
        if at >= self.ask_open_ms {
            return true;
        }
        self.ask_open_ms = self
            .ask_open_ms
            .max(at.saturating_add(termcore::menu::SETTLE_MS));
        false
    }

    /// Any key but `y`: the card is no longer armed, and is quiet again first.
    fn disarm(&mut self) {
        if let Some((ask, _)) = &mut self.ask
            && ask.armed
        {
            ask.armed = false;
            let ask = *ask;
            self.damage
                .add(trusted::card(self.width, self.height, &ask));
        }
        self.ask_open_ms = self
            .ask_open_ms
            .max(self.now_ms.saturating_add(termcore::menu::SETTLE_MS));
    }

    /// The card up now.
    pub fn ask(&self) -> Option<Ask> {
        self.ask.map(|(ask, _)| ask)
    }

    pub(super) fn end_ask(&mut self, effects: &mut Vec<Effect>) {
        let Some((ask, back)) = self.ask.take() else {
            return;
        };
        self.damage
            .add(trusted::card(self.width, self.height, &ask));
        let back = match back {
            Focus::Window(id) if self.window(id).is_none() => Focus::Nothing,
            other => other,
        };
        self.set_focus(back, effects);
    }

    /// Input while a card is up: y or n, or a click on its buttons. Nothing
    /// else goes anywhere, and the pointer still moves.
    pub(super) fn ask_input(&mut self, input: Input, repeat: bool, effects: &mut Vec<Effect>) {
        let Some((ask, _)) = self.ask else {
            return;
        };
        let answer = match input {
            Input::Motion(p) => {
                self.pointer = Point {
                    x: p.x.clamp(0, self.width - 1),
                    y: p.y.clamp(0, self.height - 1),
                };
                let (yes, _) = trusted::buttons(trusted::card(self.width, self.height, &ask));
                let since_drawn = self.ask_drawn.is_some_and(|d| self.now_ms >= d);
                if since_drawn && yes.contains(self.pointer) {
                    self.ask_entered_run = true;
                }
                None
            }
            Input::Key {
                usage: code,
                down: false,
            } => {
                self.held.retain(|&k| k != code);
                None
            }
            Input::Key {
                usage: code,
                down: true,
            } => {
                let open = self.pressed();
                match code {
                    _ if !open || repeat || self.held.contains(&code) => None,
                    usage::Y if ask.armed => Some(true),
                    usage::Y => {
                        self.arm();
                        None
                    }
                    usage::N => Some(false),
                    _ => {
                        self.disarm();
                        None
                    }
                }
            }
            Input::Button {
                button: Button::Left,
                down: true,
            } if self.pressed() => {
                let (yes, no) = trusted::buttons(trusted::card(self.width, self.height, &ask));
                // Run only for a pointer brought there after the card was
                // drawn: one parked where Run appeared clicks nothing.
                let answer = if yes.contains(self.pointer) && self.ask_entered_run {
                    Some(true)
                } else if no.contains(self.pointer) {
                    Some(false)
                } else {
                    None
                };
                // A click that answers nothing is a press like a stray key.
                if answer.is_none() {
                    self.disarm();
                }
                answer
            }
            // Any other button pressed is a press too: it disarms the card
            // and starts the quiet time again.
            Input::Button { down: true, .. } => {
                if self.pressed() {
                    self.disarm();
                }
                None
            }
            Input::Button { .. } => None,
        };
        if let Some(yes) = answer {
            effects.push(Effect::Answer { id: ask.id, yes });
            self.end_ask(effects);
        }
    }

    /// A first `y`: the card waits for a second, and says so.
    fn arm(&mut self) {
        if let Some((ask, _)) = &mut self.ask {
            ask.armed = true;
            let ask = *ask;
            self.damage
                .add(trusted::card(self.width, self.height, &ask));
        }
    }

    /// The agent acts on `window` now, or on nothing: its outline moves there.
    pub fn set_agent(&mut self, window: Option<WindowId>) {
        for id in [self.agent, window].into_iter().flatten() {
            if let Some(w) = self.window(id) {
                let outline = trusted::outline(w.frame);
                self.damage.add(outline);
            }
        }
        self.agent = window;
    }

    pub fn agent(&self) -> Option<WindowId> {
        self.agent
    }

    /// A key while the agent's window has focus, or a press on that window,
    /// is the user taking over: the run stops and the outline goes.
    pub(super) fn takeover(&mut self, input: Input, effects: &mut Vec<Effect>) {
        let Some(agent) = self.agent else {
            return;
        };
        let took = match input {
            Input::Key { down: true, .. } => self.focus == Focus::Window(agent),
            Input::Button { down: true, .. } => {
                matches!(self.hit(self.pointer), Hit::Window(id, _) if id == agent)
            }
            _ => false,
        };
        if took {
            self.set_agent(None);
            effects.push(Effect::TookOver { window: agent });
        }
    }
}
