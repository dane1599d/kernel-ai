//! What each row of the screen is, read from its text alone: the painter colours a
//! row by its kind, so any damaged row repaints the same way however it got there,
//! and the text itself (what serial and `screenlog` copy) never changes.
//!
//! The kinds follow the terminal's own conventions: `= `, `! ` and `? ` lead the
//! shell's answers, errors and questions; a prompt ends in `> `; kernel log lines
//! start with a lowercase `tag:`. A row that starts with `[` is a program's own
//! output (`[name] ...`), which no kernel line starts with: it is always a log row,
//! whatever follows, so ring-3 text can never take a prompt's or a question's look.

/// The kind of a screen row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Kernel output, after a `tag:` this many cells wide (0 for none).
    Log { tag: usize },
    /// `Kernel AI ...`: the boot banner and the terminal's greeting.
    Banner,
    /// A prompt this many cells wide, then the text typed after it.
    Input { prompt: usize },
    /// `= `: an answer from the shell or the AI.
    Reply,
    /// `ai: `: an answer `:ask` got straight from the model.
    Ai,
    /// `! `: an error.
    Error,
    /// `? `: a question that waits for yes or no.
    Question,
    /// `boot-ok` and `Ready on ...`.
    Success,
}

/// What a cell shows, which the painter turns into a colour and a weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Typed text.
    Text,
    /// Kernel output.
    Muted,
    /// A log line's `tag:`.
    Tag,
    Prompt,
    Banner,
    Reply,
    Error,
    Question,
    Success,
    /// A `= `, `! ` or `? ` lead, which the painter draws as a bar instead.
    Lead,
}

/// Cells a `= `, `! ` or `? ` lead takes.
pub const LEAD: usize = 2;
/// Cells of `ai:`.
const AI_TAG: usize = 3;
/// Longest log tag, colon included.
const MAX_TAG: usize = 16;

impl Kind {
    /// The kind of a row with these cells (`'\0'` and `' '` are blank).
    pub fn of(row: &[char]) -> Kind {
        if starts(row, "[") {
            Kind::Log { tag: 0 }
        } else if starts(row, "= ") {
            Kind::Reply
        } else if starts(row, "! ") {
            Kind::Error
        } else if starts(row, "? ") {
            Kind::Question
        } else if starts(row, "ai: ") {
            Kind::Ai
        } else if let Some(prompt) = prompt_width(row) {
            Kind::Input { prompt }
        } else if starts(row, "Kernel AI ") {
            Kind::Banner
        } else if reads(row, "boot-ok") || starts(row, "Ready on ") {
            Kind::Success
        } else {
            Kind::Log {
                tag: tag_width(row),
            }
        }
    }

    /// How the cell in column `col` shows.
    pub fn tone(self, col: usize) -> Tone {
        match self {
            Kind::Log { tag } if col < tag => Tone::Tag,
            Kind::Log { .. } => Tone::Muted,
            Kind::Banner => Tone::Banner,
            Kind::Input { prompt } if col < prompt => Tone::Prompt,
            Kind::Input { .. } => Tone::Text,
            Kind::Reply | Kind::Error | Kind::Question if col < LEAD => Tone::Lead,
            Kind::Reply => Tone::Reply,
            Kind::Ai if col < AI_TAG => Tone::Prompt,
            Kind::Ai => Tone::Reply,
            Kind::Error => Tone::Error,
            Kind::Question => Tone::Question,
            Kind::Success => Tone::Success,
        }
    }
}

fn starts(row: &[char], text: &str) -> bool {
    let mut cells = row.iter();
    text.chars().all(|ch| cells.next() == Some(&ch))
}

/// Whether the row is `text` and blanks.
fn reads(row: &[char], text: &str) -> bool {
    let n = text.chars().count();
    starts(row, text) && row.iter().skip(n).all(|&ch| ch == ' ' || ch == '\0')
}

/// The width of the prompt that starts the row, through its `> `: `kai> `,
/// `edit <name>> `, or a question's `... yes/no> ` or `... y/n> `.
fn prompt_width(row: &[char]) -> Option<usize> {
    let end = row.windows(2).position(|pair| pair == ['>', ' '])?;
    let head = &row[..end];
    let is_kai = head == ['k', 'a', 'i'];
    let is_edit = starts(head, "edit ") && head.len() > 5 && !head[5..].contains(&' ');
    let is_question = head.ends_with(&['y', 'e', 's', '/', 'n', 'o'])
        || head.ends_with(&['?', ' ', 'y', '/', 'n']);
    (is_kai || is_edit || is_question).then_some(end + 2)
}

/// The width of a log line's lowercase `tag:` and its colon, followed by a blank or
/// the end of the row; 0 if it has none.
fn tag_width(row: &[char]) -> usize {
    let Some(colon) = row.iter().take(MAX_TAG).position(|&ch| ch == ':') else {
        return 0;
    };
    let word = &row[..colon];
    let first_lower = word.first().is_some_and(char::is_ascii_lowercase);
    let tag_chars = word
        .iter()
        .all(|&ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    let ends = matches!(row.get(colon + 1), None | Some(' ' | '\0'));
    if first_lower && tag_chars && ends {
        colon + 1
    } else {
        0
    }
}

/// How a whole row looks: its background and, for answers, errors and
/// questions, a bar in the margin. The console's and the terminal window's
/// colours are `look::console`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub background: u32,
    pub bar: Option<u32>,
}

/// The look of a row of `kind`. Only answers, errors and questions differ from
/// the plain screen, so most rows repaint just the cells that change.
pub fn look(kind: Kind) -> Look {
    use look::console::*;
    let (background, bar) = match kind {
        Kind::Reply | Kind::Ai => (REPLY_TINT, Some(EMBER)),
        Kind::Error => (ERROR_TINT, Some(CORAL)),
        Kind::Question => (QUESTION_TINT, Some(GOLD)),
        _ => (INK, None),
    };
    Look { background, bar }
}

/// A cell's colour and whether it is bold; None for a lead the bar replaces.
pub fn ink(tone: Tone) -> Option<(u32, bool)> {
    use look::console::*;
    Some(match tone {
        Tone::Text => (PAPER, false),
        Tone::Muted => (SLATE, false),
        Tone::Tag => (DIM, false),
        Tone::Prompt | Tone::Banner => (EMBER, true),
        Tone::Reply => (REPLY_TEXT, false),
        Tone::Error => (ERROR_TEXT, false),
        Tone::Question => (QUESTION_TEXT, false),
        Tone::Success => (SAGE, false),
        Tone::Lead => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn kind(text: &str) -> Kind {
        let mut cells: Vec<char> = text.chars().collect();
        cells.resize(40, '\0');
        Kind::of(&cells)
    }

    #[test]
    fn shell_leads_pick_reply_error_and_question() {
        assert_eq!(kind("= 42"), Kind::Reply);
        assert_eq!(kind("! no such command"), Kind::Error);
        assert_eq!(kind("? run it? yes/no"), Kind::Question);
        assert_eq!(Kind::Reply.tone(1), Tone::Lead);
        assert_eq!(Kind::Reply.tone(2), Tone::Reply);
    }

    #[test]
    fn an_ask_answer_is_the_models_own() {
        assert_eq!(kind("ai: hello there"), Kind::Ai);
        assert_eq!(Kind::Ai.tone(2), Tone::Prompt);
        assert_eq!(Kind::Ai.tone(4), Tone::Reply);
        assert_eq!(kind("air: in"), Kind::Log { tag: 4 });
    }

    #[test]
    fn prompts_end_at_their_marker() {
        assert_eq!(kind("kai> hello"), Kind::Input { prompt: 5 });
        assert_eq!(kind("kai> "), Kind::Input { prompt: 5 });
        assert_eq!(kind("edit notes> undo"), Kind::Input { prompt: 12 });
        assert_eq!(kind("run script 3? yes/no> no"), Kind::Input { prompt: 22 });
        assert_eq!(kind("run script 3? y/n> y"), Kind::Input { prompt: 19 });
        assert_eq!(kind("x y/n> z"), Kind::Log { tag: 0 });
        let input = kind("kai> hi");
        assert_eq!(input.tone(4), Tone::Prompt);
        assert_eq!(input.tone(5), Tone::Text);
    }

    #[test]
    fn a_greater_than_inside_a_log_line_is_no_prompt() {
        assert_eq!(kind("cpu: a > b"), Kind::Log { tag: 4 });
        assert_eq!(kind("x -> y> z"), Kind::Log { tag: 0 });
    }

    #[test]
    fn log_tags_are_one_lowercase_word() {
        assert_eq!(kind("smp: 4 CPU(s) online"), Kind::Log { tag: 4 });
        assert_eq!(kind("boot-swap: done"), Kind::Log { tag: 10 });
        assert_eq!(kind("framebuffer: 1280x800"), Kind::Log { tag: 12 });
        assert_eq!(kind("Shell: no"), Kind::Log { tag: 0 });
        assert_eq!(kind("a b: c"), Kind::Log { tag: 0 });
        assert_eq!(kind("time 12:30"), Kind::Log { tag: 0 });
        assert_eq!(kind("plain text"), Kind::Log { tag: 0 });
        assert_eq!(kind(""), Kind::Log { tag: 0 });
        assert_eq!(Kind::Log { tag: 4 }.tone(3), Tone::Tag);
        assert_eq!(Kind::Log { tag: 4 }.tone(4), Tone::Muted);
    }

    #[test]
    fn a_programs_row_is_a_log_row_whatever_it_imitates() {
        assert_eq!(kind("[app] run script 9? y/n> "), Kind::Log { tag: 0 });
        assert_eq!(kind("[app] kai> rm"), Kind::Log { tag: 0 });
        assert_eq!(kind("[x] = Done"), Kind::Log { tag: 0 });
        assert_eq!(kind("[shell] ? go ahead? y/n"), Kind::Log { tag: 0 });
        assert_eq!(kind("[llm] up (restart 0)"), Kind::Log { tag: 0 });
        assert_eq!(Kind::Log { tag: 0 }.tone(0), Tone::Muted);
    }

    #[test]
    fn banner_and_success_lines() {
        assert_eq!(kind("Kernel AI 0.10.0 (M10a, commit x)"), Kind::Banner);
        assert_eq!(kind("boot-ok"), Kind::Success);
        assert_eq!(kind("boot-okay"), Kind::Log { tag: 0 });
        assert_eq!(kind("Ready on 4 CPU(s)."), Kind::Success);
    }
}
