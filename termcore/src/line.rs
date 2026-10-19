//! The line typed at a prompt, edited by keys at its cursor. Positions count
//! characters, not bytes.

use crate::keys::Key;
use alloc::string::String;

/// What a key did to the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to redraw.
    Unchanged,
    /// The text or the cursor moved.
    Changed,
    /// Enter: the finished line. The line starts again empty.
    Submitted(String),
    /// Ctrl-C: the text dropped. The line starts again empty.
    Cleared(String),
}

pub struct Line {
    text: String,
    cursor: usize,
    max: usize,
}

impl Line {
    /// An empty line that takes at most `max` characters.
    pub fn new(max: usize) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            max,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor's place in the text, in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Left, Right, Home and End move the cursor; a character is inserted at it,
    /// Backspace deletes before it and Delete at it. Enter and Ctrl-C end the line.
    pub fn apply(&mut self, key: Key) -> Outcome {
        let len = self.text.chars().count();
        match key {
            Key::Char(ch) if len < self.max => {
                self.text.insert(self.byte_at(self.cursor), ch);
                self.cursor += 1;
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.byte_at(self.cursor));
            }
            Key::Delete if self.cursor < len => {
                self.text.remove(self.byte_at(self.cursor));
            }
            Key::Left if self.cursor > 0 => self.cursor -= 1,
            Key::Right if self.cursor < len => self.cursor += 1,
            Key::Home if self.cursor > 0 => self.cursor = 0,
            Key::End if self.cursor < len => self.cursor = len,
            Key::Enter => {
                self.cursor = 0;
                return Outcome::Submitted(core::mem::take(&mut self.text));
            }
            Key::Ctrl('c') => {
                self.cursor = 0;
                return Outcome::Cleared(core::mem::take(&mut self.text));
            }
            _ => return Outcome::Unchanged,
        }
        Outcome::Changed
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(at, _)| at)
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, Outcome};
    use crate::keys::{Key, parse_tokens};
    use alloc::vec::Vec;

    fn typed(tokens: &str) -> (Line, Vec<Outcome>) {
        let mut line = Line::new(200);
        let outcomes = parse_tokens(tokens)
            .unwrap()
            .into_iter()
            .map(|key| line.apply(key))
            .collect();
        (line, outcomes)
    }

    #[test]
    fn keys_edit_at_the_cursor() {
        let (line, _) = typed("helo<left>l");
        assert_eq!((line.text(), line.cursor()), ("hello", 4));
        let (line, _) = typed("ac<home>b<home><right>!<end>?");
        assert_eq!((line.text(), line.cursor()), ("b!ac?", 5));
        let (line, _) = typed("abc<left><left><bs><del>");
        assert_eq!((line.text(), line.cursor()), ("c", 0));
    }

    #[test]
    fn moves_stop_at_the_ends() {
        let (line, outcomes) = typed("<left><home><bs>ab<right><end><del>");
        assert_eq!((line.text(), line.cursor()), ("ab", 2));
        let changed: Vec<bool> = outcomes.iter().map(|o| *o == Outcome::Changed).collect();
        assert_eq!(
            changed,
            [false, false, false, true, true, false, false, false]
        );
    }

    #[test]
    fn insertion_stops_at_the_cap() {
        let mut line = Line::new(3);
        for ch in "abcd".chars() {
            line.apply(Key::Char(ch));
        }
        assert_eq!(line.text(), "abc");
        assert_eq!(line.apply(Key::Left), Outcome::Changed);
        assert_eq!(line.apply(Key::Char('x')), Outcome::Unchanged);
    }

    #[test]
    fn enter_and_ctrl_c_end_the_line() {
        let (line, outcomes) = typed("ab<enter>cd<C-c>");
        assert_eq!(outcomes[2], Outcome::Submitted("ab".into()));
        assert_eq!(outcomes[5], Outcome::Cleared("cd".into()));
        assert_eq!((line.text(), line.cursor()), ("", 0));
    }

    #[test]
    fn other_keys_do_nothing_at_the_prompt() {
        let (line, outcomes) = typed("a<up><down><pgup><pgdn><esc><tab><C-b>");
        assert_eq!((line.text(), line.cursor()), ("a", 1));
        assert!(outcomes[1..].iter().all(|o| *o == Outcome::Unchanged));
    }

    #[test]
    fn the_boot_case_keys_clear_a_line_and_edit_policy_rr() {
        let (_, outcomes) = typed(
            "xyz<C-c>polcy rrxz<home><up><pgdn><esc><right><right><right>i\
             <end><bs><left><del><enter>",
        );
        let ends: Vec<&Outcome> = outcomes
            .iter()
            .filter(|o| !matches!(o, Outcome::Changed | Outcome::Unchanged))
            .collect();
        assert_eq!(
            ends,
            [
                &Outcome::Cleared("xyz".into()),
                &Outcome::Submitted("policy rr".into())
            ]
        );
        // A Delete that acted as Backspace would not give "policy rr".
        let (line, _) = typed("policy rrx<left><bs>");
        assert_eq!(line.text(), "policy rx");
    }

    #[test]
    fn positions_count_characters() {
        let mut line = Line::new(10);
        for key in [
            Key::Char('\u{e9}'),
            Key::Char('b'),
            Key::Left,
            Key::Backspace,
        ] {
            line.apply(key);
        }
        assert_eq!((line.text(), line.cursor()), ("b", 0));
    }
}
