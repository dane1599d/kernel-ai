//! The trusted layer's rules (ADR 0005 §7): the ask card takes the keyboard
//! and only y, n or a click on its buttons answers it; a key or click by the
//! user in the agent's window takes over from the agent.

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
        let ask = Ask { id, lines };
        self.ask = Some((ask, back));
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
    pub(super) fn ask_input(&mut self, input: Input, effects: &mut Vec<Effect>) {
        let Some((ask, _)) = self.ask else {
            return;
        };
        let answer = match input {
            Input::Motion(p) => {
                self.pointer = Point {
                    x: p.x.clamp(0, self.width - 1),
                    y: p.y.clamp(0, self.height - 1),
                };
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
            } if !self.held.contains(&code) => match code {
                usage::Y => Some(true),
                usage::N => Some(false),
                _ => None,
            },
            Input::Key { .. } => None,
            Input::Button {
                button: Button::Left,
                down: true,
            } => {
                let (yes, no) = trusted::buttons(trusted::card(self.width, self.height, &ask));
                if yes.contains(self.pointer) {
                    Some(true)
                } else if no.contains(self.pointer) {
                    Some(false)
                } else {
                    None
                }
            }
            Input::Button { .. } => None,
        };
        if let Some(yes) = answer {
            effects.push(Effect::Answer { id: ask.id, yes });
            self.end_ask(effects);
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
