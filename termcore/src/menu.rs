//! The Allow / Deny menu (ADR 0002 D2a), drawn only by the kernel terminal so no
//! program or model output can draw one: a call that asks shows its question and
//! its choices, and a key picks one.
//!
//! ```text
//! ? Stop process "build-3"? It has no undo.
//! ?   1 Allow
//! ?   2 Deny
//! ?   3 Always allow stopping processes
//! ?   4 Always allow stopping "build-3"
//! allow? 1 Allow>
//! ```
//!
//! The rows are printed once, as `?` rows; the last line is the terminal's input
//! region, which shows the highlighted choice and moves with the arrows. Up,
//! Down, Home and End move; Enter takes the highlighted choice; a digit takes
//! that choice; `y` is Allow and `n` is Deny; Esc and Ctrl-C are Deny. Keys
//! typed before the menu showed are dropped: it takes none until [`Menu::arm`].
//! The highlight starts on Allow for a call the user plainly asked for, and on
//! Deny for one the AI added or that data steered. The "Always allow" rows come
//! only when a standing rule may waive this ask; the shell decides that.
//!
//! On the wire the shell sends `menu <id> <asked|added>` and then one line each
//! for the question, the kind rule and the target rule (either may be empty),
//! and the terminal answers `answer <id> allow|deny|always-kind|always-target`.

use crate::keys::Key;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Longest question, rule label or line.
pub const MAX_TEXT: usize = 120;
/// The input region's prompt while a menu waits.
const PROMPT: &str = "allow? ";

/// What the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
    /// A standing rule for the action's kind ("stopping processes").
    AlwaysKind,
    /// A standing rule for this target ("stopping \"build-3\"").
    AlwaysTarget,
}

impl Answer {
    /// The word the terminal sends back.
    pub fn word(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::AlwaysKind => "always-kind",
            Self::AlwaysTarget => "always-target",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::Allow,
            Self::Deny,
            Self::AlwaysKind,
            Self::AlwaysTarget,
        ]
        .into_iter()
        .find(|answer| answer.word() == word)
    }
}

/// Where the call came from, which sets where the highlight starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The user's line plainly asked for it: the highlight starts on Allow.
    Asked,
    /// The AI added it or data steered it: the highlight starts on Deny.
    Added,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing: a key the menu does not take, or one before it was armed.
    Ignored,
    /// The highlight moved; redraw the input region.
    Moved,
    Chosen(Answer),
}

/// Why a menu request was refused. The terminal then denies the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Malformed,
    /// A text was empty, too long or not printable ASCII, so it could not be
    /// shown exactly as the shell sent it.
    Unshowable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub id: u64,
    question: String,
    /// The call the question shows, canonical (`termcore::once::call_text`): what
    /// an Allow's ONCE digest names (`once::digest_call`).
    call: String,
    choices: Vec<(Answer, String)>,
    selected: usize,
    armed: bool,
}

fn showable(text: &str) -> bool {
    text.len() <= MAX_TEXT && text.bytes().all(|b| (b' '..=b'~').contains(&b))
}

impl Menu {
    /// A menu for `question`. `kind` and `target` label the standing rules a
    /// choice may write ("stopping processes"); each is offered only when given.
    pub fn new(
        id: u64,
        question: &str,
        kind: Option<&str>,
        target: Option<&str>,
        origin: Origin,
    ) -> Result<Self, Refused> {
        let labels = [Some(question), kind, target];
        if question.is_empty() || !labels.into_iter().flatten().all(showable) {
            return Err(Refused::Unshowable);
        }
        let mut choices = alloc::vec![
            (Answer::Allow, String::from("Allow")),
            (Answer::Deny, String::from("Deny")),
        ];
        let rules = [(Answer::AlwaysKind, kind), (Answer::AlwaysTarget, target)];
        for (answer, label) in rules {
            if let Some(label) = label.filter(|l| !l.is_empty()) {
                choices.push((answer, format!("Always allow {label}")));
            }
        }
        if choices.iter().any(|(_, label)| label.len() > MAX_TEXT) {
            return Err(Refused::Unshowable);
        }
        Ok(Self {
            id,
            question: question.to_string(),
            call: String::new(),
            choices,
            selected: match origin {
                Origin::Asked => 0,
                Origin::Added => 1,
            },
            armed: false,
        })
    }

    /// The menu for `call`, which `question` must show word for word: the ONCE an
    /// Allow mints names this call, so it must be what the user saw.
    pub fn with_call(self, call: &str) -> Result<Self, Refused> {
        if call.is_empty() || !showable(call) || !self.question.contains(call) {
            return Err(Refused::Unshowable);
        }
        Ok(Self {
            call: call.to_string(),
            ..self
        })
    }

    /// The call the question shows.
    pub fn call(&self) -> &str {
        &self.call
    }

    /// Reads `menu <id> <asked|added>`, then the question, the kind rule, the
    /// target rule and the call, one line each.
    pub fn decode(text: &str) -> Result<Self, Refused> {
        let mut lines = text.split('\n');
        let header = lines.next().ok_or(Refused::Malformed)?;
        let mut words = header
            .strip_prefix("menu ")
            .ok_or(Refused::Malformed)?
            .split(' ');
        let id = words
            .next()
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or(Refused::Malformed)?;
        let origin = match words.next() {
            Some("asked") => Origin::Asked,
            Some("added") => Origin::Added,
            _ => return Err(Refused::Malformed),
        };
        let (Some(question), Some(kind), Some(target), Some(call), None, None) = (
            lines.next(),
            lines.next(),
            lines.next(),
            lines.next(),
            lines.next(),
            words.next(),
        ) else {
            return Err(Refused::Malformed);
        };
        let given = |label: &str| Some(label).filter(|l| !l.is_empty()).map(String::from);
        let (kind, target) = (given(kind), given(target));
        Self::new(id, question, kind.as_deref(), target.as_deref(), origin)?.with_call(call)
    }

    /// The shell's side of [`Menu::decode`].
    pub fn encode(
        id: u64,
        origin: Origin,
        question: &str,
        kind: &str,
        target: &str,
        call: &str,
    ) -> String {
        let origin = match origin {
            Origin::Asked => "asked",
            Origin::Added => "added",
        };
        format!("menu {id} {origin}\n{question}\n{kind}\n{target}\n{call}")
    }

    /// The `?` rows, printed once: the question, then each choice numbered.
    pub fn rows(&self) -> Vec<String> {
        let mut rows = alloc::vec![format!("? {}", self.question)];
        rows.extend(
            self.choices
                .iter()
                .enumerate()
                .map(|(i, (_, label))| format!("?   {} {label}", i + 1)),
        );
        rows
    }

    /// The input region's prompt and text: the highlighted choice.
    pub fn input(&self) -> (&'static str, String) {
        let (_, label) = &self.choices[self.selected];
        (PROMPT, format!("{} {label}", self.selected + 1))
    }

    /// The question, exactly as shown: what an Allow's `ONCE` digest names.
    pub fn question(&self) -> &str {
        &self.question
    }

    /// The highlighted choice.
    pub fn selected(&self) -> Answer {
        self.choices[self.selected].0
    }

    /// From now on keys count: call it once the rows and the input region are on
    /// screen and the keys typed before are dropped.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// What `key` does.
    pub fn apply(&mut self, key: Key) -> Step {
        if !self.armed {
            return Step::Ignored;
        }
        let last = self.choices.len() - 1;
        let to = match key {
            Key::Up | Key::Left => self.selected.saturating_sub(1),
            Key::Down | Key::Right | Key::Tab => (self.selected + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::Enter => return Step::Chosen(self.selected()),
            Key::Char('y' | 'Y') => return Step::Chosen(Answer::Allow),
            Key::Char('n' | 'N') | Key::Esc | Key::Ctrl('c') => return Step::Chosen(Answer::Deny),
            Key::Char(digit @ '1'..='9') => {
                let n = digit as usize - '1' as usize;
                return match self.choices.get(n) {
                    Some((answer, _)) => Step::Chosen(*answer),
                    None => Step::Ignored,
                };
            }
            _ => return Step::Ignored,
        };
        match to == self.selected {
            true => Step::Ignored,
            false => {
                self.selected = to;
                Step::Moved
            }
        }
    }
}

/// The terminal's answer line.
pub fn answer_line(id: u64, answer: Answer) -> String {
    format!("answer {id} {}", answer.word())
}

/// What the trusted path minted for a choice (ADR 0001 D6): the `ONCE` badge an
/// Allow gives, and the `Grant` handle in the shell's table for the rule node an
/// "Always allow" made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Granted {
    pub once: Option<u64>,
    pub rule: Option<u32>,
}

/// The answer line with what was minted: `answer <id> allow once=<badge>`, and
/// ` rule=<handle>` after an "Always allow".
pub fn granted_line(id: u64, answer: Answer, granted: Granted) -> String {
    let mut line = answer_line(id, answer);
    if let Some(badge) = granted.once {
        line.push_str(&format!(" once={badge}"));
    }
    if let Some(handle) = granted.rule {
        line.push_str(&format!(" rule={handle}"));
    }
    line
}

/// Reads what follows `answer <id> `: the choice, then what was minted. `None`
/// for anything else, so a typed `y` stays a typed `y`.
pub fn read_answer(text: &str) -> Option<(Answer, Granted)> {
    let mut words = text.split(' ');
    let answer = Answer::from_word(words.next()?)?;
    let mut granted = Granted::default();
    for word in words {
        match word.split_once('=') {
            Some(("once", n)) if granted.once.is_none() => granted.once = Some(n.parse().ok()?),
            Some(("rule", n)) if granted.rule.is_none() => granted.rule = Some(n.parse().ok()?),
            _ => return None,
        }
    }
    Some((answer, granted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(origin: Origin) -> Menu {
        let mut menu = Menu::new(
            7,
            "Stop process \"build-3\"? It has no undo.",
            Some("stopping processes"),
            Some("stopping \"build-3\""),
            origin,
        )
        .unwrap();
        menu.arm();
        menu
    }

    #[test]
    fn it_shows_the_question_and_numbered_choices() {
        let menu = stop(Origin::Asked);
        assert_eq!(
            menu.rows(),
            [
                "? Stop process \"build-3\"? It has no undo.",
                "?   1 Allow",
                "?   2 Deny",
                "?   3 Always allow stopping processes",
                "?   4 Always allow stopping \"build-3\"",
            ]
        );
        assert_eq!(menu.input(), ("allow? ", String::from("1 Allow")));
        // Every row is a question row to the painter.
        for row in menu.rows() {
            let cells: Vec<char> = row.chars().collect();
            assert_eq!(crate::style::Kind::of(&cells), crate::style::Kind::Question);
        }
    }

    #[test]
    fn the_highlight_starts_on_deny_for_an_added_call() {
        assert_eq!(stop(Origin::Asked).selected(), Answer::Allow);
        let mut added = stop(Origin::Added);
        assert_eq!(added.selected(), Answer::Deny);
        assert_eq!(added.apply(Key::Enter), Step::Chosen(Answer::Deny));
    }

    #[test]
    fn arrows_move_and_enter_takes_the_highlight() {
        let mut menu = stop(Origin::Asked);
        assert_eq!(menu.apply(Key::Up), Step::Ignored);
        assert_eq!(menu.apply(Key::Down), Step::Moved);
        assert_eq!(menu.apply(Key::Down), Step::Moved);
        assert_eq!(menu.input().1, "3 Always allow stopping processes");
        assert_eq!(menu.apply(Key::End), Step::Moved);
        assert_eq!(menu.apply(Key::Down), Step::Ignored);
        assert_eq!(menu.apply(Key::Enter), Step::Chosen(Answer::AlwaysTarget));
        assert_eq!(menu.apply(Key::Home), Step::Moved);
        assert_eq!(menu.apply(Key::Enter), Step::Chosen(Answer::Allow));
    }

    #[test]
    fn y_n_digits_esc_and_ctrl_c_answer_at_once() {
        let mut menu = stop(Origin::Added);
        assert_eq!(menu.apply(Key::Char('y')), Step::Chosen(Answer::Allow));
        assert_eq!(menu.apply(Key::Char('N')), Step::Chosen(Answer::Deny));
        assert_eq!(menu.apply(Key::Esc), Step::Chosen(Answer::Deny));
        assert_eq!(menu.apply(Key::Ctrl('c')), Step::Chosen(Answer::Deny));
        assert_eq!(menu.apply(Key::Char('3')), Step::Chosen(Answer::AlwaysKind));
        assert_eq!(menu.apply(Key::Char('5')), Step::Ignored);
        assert_eq!(menu.apply(Key::Char('x')), Step::Ignored);
        assert_eq!(menu.apply(Key::Backspace), Step::Ignored);
    }

    #[test]
    fn keys_before_it_is_armed_are_dropped() {
        let mut menu = Menu::new(1, "Reboot?", None, None, Origin::Asked).unwrap();
        for key in [Key::Char('y'), Key::Enter, Key::Down] {
            assert_eq!(menu.apply(key), Step::Ignored);
        }
        menu.arm();
        assert_eq!(menu.apply(Key::Enter), Step::Chosen(Answer::Allow));
    }

    #[test]
    fn rules_are_offered_only_when_given() {
        let menu = Menu::new(1, "Reboot? It has no undo.", None, None, Origin::Asked).unwrap();
        assert_eq!(menu.rows().len(), 3);
        let mut menu = Menu::new(1, "q", None, Some("stopping \"x\""), Origin::Asked).unwrap();
        menu.arm();
        assert_eq!(menu.rows()[3], "?   3 Always allow stopping \"x\"");
        assert_eq!(
            menu.apply(Key::Char('3')),
            Step::Chosen(Answer::AlwaysTarget)
        );
        assert_eq!(menu.apply(Key::Char('4')), Step::Ignored);
    }

    #[test]
    fn the_wire_round_trips_and_refuses_what_it_cannot_show() {
        let text = Menu::encode(
            9,
            Origin::Added,
            "Run stop(1)?",
            "stopping processes",
            "",
            "stop(1)",
        );
        let menu = Menu::decode(&text).unwrap();
        assert_eq!(menu.id, 9);
        assert_eq!(menu.call(), "stop(1)");
        assert_eq!(menu.selected(), Answer::Deny);
        assert_eq!(menu.rows().len(), 4);
        for bad in [
            "",
            "menu",
            "menu x asked\nq\n\n\nq",
            "menu 1 maybe\nq\n\n\nq",
            "menu 1 asked\nq\n\n",
            "menu 1 asked\nq\n\n\nq\n",
            "menu 1 asked extra\nq\n\n\nq",
            "confirm 1 asked\nq\n\n\nq",
        ] {
            assert_eq!(Menu::decode(bad), Err(Refused::Malformed), "{bad:?}");
        }
        let long = "x".repeat(MAX_TEXT + 1);
        for bad in [
            Menu::encode(1, Origin::Asked, "", "", "", "q"),
            Menu::encode(1, Origin::Asked, "q\u{1b}[2J", "", "", "q"),
            Menu::encode(1, Origin::Asked, "q", "\u{e9}", "", "q"),
            Menu::encode(1, Origin::Asked, &long, "", "", "x"),
            Menu::encode(1, Origin::Asked, "q", &"k".repeat(MAX_TEXT - 5), "", "q"),
            // The call must be there, and be what the question shows.
            Menu::encode(1, Origin::Asked, "Run stop(1)?", "", "", ""),
            Menu::encode(1, Origin::Asked, "Run stop(1)?", "", "", "stop(2)"),
        ] {
            assert_eq!(Menu::decode(&bad), Err(Refused::Unshowable), "{bad:?}");
        }
        assert_eq!(answer_line(9, Answer::AlwaysKind), "answer 9 always-kind");
        for answer in [
            Answer::Allow,
            Answer::Deny,
            Answer::AlwaysKind,
            Answer::AlwaysTarget,
        ] {
            assert_eq!(Answer::from_word(answer.word()), Some(answer));
        }
        assert_eq!(Answer::from_word("yes"), None);
    }

    /// Hostile wire text, cut at every byte and mutated: it decodes or is
    /// refused, never panics, and what decodes has showable rows.
    #[test]
    fn hostile_menus_decode_or_are_refused() {
        let good = Menu::encode(
            3,
            Origin::Asked,
            "Stop \"b\"?",
            "stopping",
            "stopping \"b\"",
            "\"b\"",
        );
        let bytes = good.as_bytes();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for cut in 0..=bytes.len() {
            let _ = Menu::decode(&String::from_utf8_lossy(&bytes[..cut]));
        }
        for _ in 0..4000 {
            let mut mutated = bytes.to_vec();
            for _ in 0..3 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = seed as usize % mutated.len();
                mutated[at] = (seed >> 32) as u8;
            }
            if let Ok(menu) = Menu::decode(&String::from_utf8_lossy(&mutated)) {
                assert!(
                    menu.rows()
                        .iter()
                        .all(|r| showable(r) || r.len() <= MAX_TEXT + 8)
                );
                assert!(menu.rows().len() >= 3 && menu.rows().len() <= 5);
            }
        }
    }
}

#[cfg(test)]
mod granted_tests {
    use super::*;

    #[test]
    fn what_was_minted_rides_on_the_answer_line_and_reads_back() {
        let granted = Granted {
            once: Some(0x1234),
            rule: Some(7),
        };
        let line = granted_line(3, Answer::AlwaysKind, granted);
        assert_eq!(line, "answer 3 always-kind once=4660 rule=7");
        let rest = line.strip_prefix("answer 3 ").unwrap();
        assert_eq!(read_answer(rest), Some((Answer::AlwaysKind, granted)));
        assert_eq!(
            read_answer("deny"),
            Some((Answer::Deny, Granted::default()))
        );
        for bad in [
            "y",
            "allow once=x",
            "allow once=1 once=2",
            "allow when=1",
            "",
            "allow  once=1",
        ] {
            assert_eq!(read_answer(bad), None, "{bad:?}");
        }
    }
}
