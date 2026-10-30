//! The one thing the terminal asks the OS for: a line run by the shell. It is an
//! effect (ADR 0003 section 4), `shell_line`, with the line as a WAVE record.
//! The agent core runs it with the shell's own rules (a line that would ask is
//! refused, as the AI panel's is) and the answer comes back as `effect-done`:
//! the shell's wire reply (`ok ...`, `err ...` or `more`) as one line, which
//! [`crate::Terminal::done`] shows.

use alloc::format;
use alloc::string::String;

/// The effect's name. It is not a registry row: the shell handles it for a
/// package that declares it.
pub const SHELL_LINE: &str = "shell_line";

/// Longest argument text of an effect call (`livecore::txn::MAX_EFFECT_ARGS`).
pub const MAX_ARGS: usize = 512;

/// What the terminal needs from the OS: effects queued in a call.
pub trait Os {
    /// Queues effect `name` with its arguments as a WAVE record; its ticket,
    /// which [`crate::Terminal::done`] gets with the result.
    fn request(&mut self, name: &'static str, args: String) -> u32;
}

/// A WAVE string: quoted, with `\` and `"` and control characters escaped as
/// `appabi::wave` prints them.
pub fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The arguments of `shell_line` for `line`.
pub fn shell_line_args(line: &str) -> String {
    format!("{{line: {}}}", quote(line))
}

/// The shell's one-line answer as its wire reply. The shell writes a newline as
/// backslash and `n`, and a backslash as two, so that the answer is one line
/// (`shellcore`'s `appline`); any other backslash stays.
pub fn unescape(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.clone().next() {
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// A stand-in OS for tests: it numbers the effects it is asked for.
#[derive(Debug, Default)]
pub struct Recorder {
    /// Every effect asked for, with its ticket, oldest first.
    pub asked: alloc::vec::Vec<(u32, &'static str, String)>,
    next: u32,
}

impl Os for Recorder {
    fn request(&mut self, name: &'static str, args: String) -> u32 {
        self.next += 1;
        self.asked.push((self.next, name, args));
        self.next
    }
}

impl Recorder {
    /// The lines sent to the shell, oldest first.
    pub fn lines(&self) -> alloc::vec::Vec<String> {
        self.asked
            .iter()
            .filter(|(_, name, _)| *name == SHELL_LINE)
            .map(|(_, _, args)| args.clone())
            .collect()
    }

    /// The ticket of the latest effect.
    pub fn last_ticket(&self) -> Option<u32> {
        self.asked.last().map(|(t, _, _)| *t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_SENT_BYTES;

    #[test]
    fn a_line_is_a_wave_record() {
        assert_eq!(shell_line_args("mem"), r#"{line: "mem"}"#);
        assert_eq!(
            shell_line_args(r#"say "hi" \ there"#),
            r#"{line: "say \"hi\" \\ there"}"#
        );
        assert_eq!(shell_line_args("\u{3}"), r#"{line: "\u{3}"}"#);
    }

    #[test]
    fn the_shells_one_line_answer_reads_back_as_its_wire_reply() {
        assert_eq!(unescape(r"ok a\nb\\c"), "ok a\nb\\c");
        assert_eq!(unescape("ok plain"), "ok plain");
        // A lone backslash, and one before another character, stay.
        assert_eq!(unescape(r"ok a\tb\"), r"ok a\tb\");
        assert_eq!(unescape(r"\\n"), r"\n");
        assert_eq!(unescape(""), "");
    }

    #[test]
    fn the_longest_line_fits_the_effect_limit() {
        // A line holds no control character, so only a quote or a backslash
        // takes two bytes in the record.
        for ch in ['"', '\\', 'x'] {
            let worst: String = core::iter::repeat_n(ch, MAX_SENT_BYTES).collect();
            assert!(shell_line_args(&worst).len() <= MAX_ARGS, "{ch:?}");
        }
    }
}
