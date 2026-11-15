//! Keys the desk itself takes (ADR 0005 §8, "Keys"): the locks, Alt+Tab,
//! a Super tap or Ctrl+Esc, which open Start with its box focused, and
//! Ctrl+Space, which opens the Kai pane and focuses its input. Every other key goes to the focus: a window, the panel's input, or
//! nowhere while a card is up.

use super::usage::{self, SPACE, TAB};

/// Esc, for Ctrl+Esc.
const ESCAPE: u16 = 0x29;
use super::{Desk, Effect, Focus};
use alloc::vec::Vec;

impl Desk {
    pub(super) fn key(&mut self, code: u16, down: bool, effects: &mut Vec<Effect>) {
        // The locks toggle on their press, and go to no one.
        if code == crate::keymap::CAPS_LOCK || code == crate::keymap::NUM_LOCK {
            if down && code == crate::keymap::CAPS_LOCK {
                self.caps = !self.caps;
                effects.push(Effect::Locks { caps: self.caps });
            } else if down {
                self.num = !self.num;
            }
            return;
        }
        // With Num Lock off the keypad is the arrows and navigation keys.
        let code = match self.num {
            true => code,
            false => match crate::keymap::keypad_off(code) {
                Some(nav) => nav,
                None if (0x59..=0x63).contains(&code) => return,
                None => code,
            },
        };
        match code {
            usage::LEFT_CTRL | usage::RIGHT_CTRL => self.ctrl = down,
            usage::LEFT_ALT | usage::RIGHT_ALT => self.alt = down,
            usage::LEFT_SHIFT | usage::RIGHT_SHIFT => self.shift = down,
            usage::LEFT_GUI | usage::RIGHT_GUI => {
                if down {
                    self.super_alone = true;
                } else if core::mem::take(&mut self.super_alone) {
                    self.toggle_start(effects);
                }
                return;
            }
            _ => {}
        }
        if down {
            self.super_alone = false;
        }
        if down && code == TAB && self.alt {
            self.cycle(effects);
            return;
        }
        if down && code == SPACE && self.ctrl {
            self.focus_pane(effects);
            return;
        }
        // Ctrl+Esc: Start, where Super may not reach the guest.
        if down && code == ESCAPE && self.ctrl {
            self.open_start(effects);
            return;
        }
        // Tab, Space and Esc released after the shortcut go nowhere.
        let shortcut =
            (code == TAB && self.alt) || ((code == SPACE || code == ESCAPE) && self.ctrl);
        if !down && shortcut {
            return;
        }
        match self.focus {
            Focus::Window(window) => effects.push(Effect::Key {
                window,
                usage: code,
                down,
            }),
            Focus::Panel => effects.push(Effect::PanelKey { usage: code, down }),
            Focus::Nothing | Focus::Ask(_) => {}
        }
    }
}
