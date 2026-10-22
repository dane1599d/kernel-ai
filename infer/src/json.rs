//! Constrained decoding for one fixed JSON shape: an object of string fields in a
//! fixed order with no spaces, `{"a":"...","b":"..."}`. Each step allows only the
//! tokens that keep the answer a prefix of such an object, so an answer that
//! closes it always parses, as a server's `response_format` would make it.
//!
//! A string holds printable ASCII and the escapes `\"`, `\\`, `\/` and `\n`;
//! nothing else, not even a tab, so the model is steered away from characters
//! the reader would refuse or rewrite (a server's answer gets a repair round for
//! one). Limits apply to the string as the reader sees it: escapes read, and
//! spaces and newlines at the end trimmed, as are those at the start of a field
//! that [`Field::trim_start`]s. They never steer the answer: a token past a limit
//! is still the model's to pick, and picking it ends the answer as an
//! [`Overflow`], which the reader refuses with its own reason, as it would refuse
//! a server's answer over that limit. Spaces and newlines past a limit end
//! nothing yet, since the reader may trim them; only more of them or the closing
//! quote may follow. Nor do the token limit and the context steer: an answer the
//! model can no longer close in what is left is cut off, conservatively, since
//! `Grammar::need` counts the bytes closing takes as one token each.
//!
//! A field may instead be a [`Choice`]: one of a fixed set of words, or with
//! `list` an array of at most that many of them, `"name":["a","b"]`. Words are
//! printable ASCII without quotes or backslashes, so a choice needs no escape,
//! and only the bytes that keep it a prefix of some word may come; a list takes
//! each word at most once. The reader still checks the answer. M10's `tune` answer
//! is built this way: a reason, then enums and a list of the table's names.
//!
//! That needs a token of one byte for every printable ASCII byte, which
//! byte-level vocabularies have.

use crate::Error;
use alloc::vec::Vec;
use bpe::{Kind, Tokenizer};

/// One string field of the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Letters, digits and `_` only, so the name needs no escaping.
    pub name: &'static str,
    /// Most characters in the string, newlines included.
    pub max_chars: usize,
    /// Most lines, so 1 allows no newline.
    pub max_lines: usize,
    /// Most characters on one line.
    pub max_line_chars: usize,
    /// The reader trims spaces and newlines at the start as well as at the end,
    /// so they count for nothing there.
    pub trim_start: bool,
    /// The limits steer: a byte past one is masked, so the model must close the
    /// string instead of ending the answer there (`tune`'s reason, which a
    /// model otherwise runs past and loses the whole answer to).
    pub hard: bool,
}

/// Makes field `field` a choice among `words`: one of them, or with `list` an
/// array of at most `list` of them. The field's string limits do not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice<'a> {
    pub field: usize,
    pub words: &'a [&'a str],
    pub list: Option<usize>,
}

/// Most words a choice may offer.
pub const MAX_WORDS: usize = 64;

/// The object: its fields, in order, and the fields that are choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape<'a> {
    pub fields: &'a [Field],
    pub choices: &'a [Choice<'a>],
    /// Jump-forward (SGLang's): the bytes the shape alone decides, the literals
    /// between the fields and at either end, go to the model as one block of
    /// their own tokens instead of one decode step each. The tokens may differ
    /// from the ones step decoding would pick, so the answer may too; it is the
    /// same answer every time, from a warm cache or a fresh one.
    pub jump_forward: bool,
}

impl Shape<'_> {
    /// Whether the shape can be decoded: at least one field, names that need no
    /// escaping and differ, and room for a character in every string.
    pub fn check(&self) -> Result<(), Error> {
        if self.fields.is_empty() {
            return Err(Error::Shape("has no fields"));
        }
        for (i, field) in self.fields.iter().enumerate() {
            let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            if field.name.is_empty() || !field.name.bytes().all(plain) {
                return Err(Error::Shape("has a field name that is not a plain word"));
            }
            if self.fields[..i].iter().any(|f| f.name == field.name) {
                return Err(Error::Shape("has two fields of one name"));
            }
            if field.max_chars == 0 || field.max_lines == 0 || field.max_line_chars == 0 {
                return Err(Error::Shape("has a string with no room for a character"));
            }
        }
        for (i, choice) in self.choices.iter().enumerate() {
            if choice.field >= self.fields.len()
                || self.choices[..i].iter().any(|c| c.field == choice.field)
            {
                return Err(Error::Shape("has a choice for no field, or two for one"));
            }
            let plain = |w: &&str| !w.is_empty() && w.bytes().all(word_byte);
            if choice.words.len() > MAX_WORDS
                || (choice.words.is_empty() && choice.list.is_none())
                || !choice.words.iter().all(plain)
                || (1..choice.words.len()).any(|j| choice.words[..j].contains(&choice.words[j]))
            {
                return Err(Error::Shape(
                    "has a choice whose words are not plain and distinct",
                ));
            }
        }
        Ok(())
    }
}

/// A byte a choice's word may hold: printable ASCII but a quote or a backslash.
fn word_byte(b: u8) -> bool {
    matches!(b, b' '..=b'~') && b != b'"' && b != b'\\'
}

/// Which limit of a field a token went past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(PartialOrd, Ord))]
pub enum Limit {
    Chars,
    Lines,
    LineChars,
}

/// The model picked a token past a limit of a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(PartialOrd, Ord))]
pub struct Overflow {
    /// The field's index in the shape.
    pub field: usize,
    pub limit: Limit,
    /// The line, from 1, the token was on.
    pub line: usize,
}

/// Where the answer is in the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(PartialOrd, Ord))]
enum At {
    /// This many bytes into the literal before the state's field: `{"name":"`
    /// before the first, `","name":"` before the others, and the closing `"}` at
    /// the field past the last.
    Lead(usize),
    /// In the field's string.
    Text,
    /// In the field's string, after a backslash.
    Escape,
    /// In a list, after `[`: an item's quote or the closing `]`.
    ItemNext,
    /// In a list, after `,`: an item's quote.
    ItemOpen,
    /// In an item's word.
    Item,
    /// In a list, after an item: `,` or `]`.
    ItemDone,
    /// The object is closed.
    Done,
}

/// How far an answer has got, with the counts the string's limits need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(PartialOrd, Ord))]
pub(crate) struct State {
    field: usize,
    at: At,
    chars: usize,
    /// Newlines in the string so far.
    lines: usize,
    line_chars: usize,
    /// The string has a character other than a space or a newline.
    seen: bool,
    /// Spaces or newlines went past this limit: the string is over it unless
    /// nothing but more of them comes before its closing quote.
    tail: Option<Overflow>,
    /// In a choice, the words the bytes so far begin, as a mask over its words.
    alive: u64,
    /// Items so far in a list, and the words they took.
    items: usize,
    used: u64,
}

impl State {
    pub(crate) fn is_done(&self) -> bool {
        self.at == At::Done
    }

    /// `at` bytes into the literal before field `field`, with fresh counts.
    fn lead(field: usize, at: usize) -> Self {
        Self {
            field,
            at: At::Lead(at),
            chars: 0,
            lines: 0,
            line_chars: 0,
            seen: false,
            tail: None,
            alive: 0,
            items: 0,
            used: 0,
        }
    }
}

/// The mask of the first `n` words.
fn all_words(n: usize) -> u64 {
    match n {
        64.. => u64::MAX,
        n => (1u64 << n) - 1,
    }
}

/// Why a byte cannot come next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// It does not fit the shape: never allowed.
    Masked,
    /// It goes past a limit of the field: allowed, and picking it ends the
    /// answer.
    Over(Overflow),
}

/// A checked shape and the fewest bytes from each field's string to the end.
pub(crate) struct Grammar<'s> {
    fields: &'s [Field],
    /// Each field's choice, if it is one.
    choices: Vec<Option<Choice<'s>>>,
    /// `suffix[k]`: the fewest bytes from the start of field k's string to the
    /// closed object, every string left empty; `suffix[fields.len()]` is 0.
    suffix: Vec<usize>,
}

impl<'s> Grammar<'s> {
    pub(crate) fn new(shape: &Shape<'s>) -> Result<Self, Error> {
        shape.check()?;
        let n = shape.fields.len();
        let mut suffix = Vec::new();
        suffix
            .try_reserve_exact(n + 1)
            .map_err(|_| Error::OutOfMemory)?;
        suffix.resize(n + 1, 0);
        let mut choices = Vec::new();
        choices
            .try_reserve_exact(n)
            .map_err(|_| Error::OutOfMemory)?;
        choices.extend((0..n).map(|k| shape.choices.iter().find(|c| c.field == k).copied()));
        let mut grammar = Self {
            fields: shape.fields,
            choices,
            suffix,
        };
        for k in (0..n).rev() {
            let shortest = match grammar.choice(k) {
                Some(c) if c.list.is_none() => c.words.iter().map(|w| w.len()).min().unwrap_or(0),
                _ => 0,
            };
            grammar.suffix[k] = shortest + grammar.lead_len(k + 1) + grammar.suffix[k + 1];
        }
        Ok(grammar)
    }

    fn choice(&self, field: usize) -> Option<&Choice<'s>> {
        self.choices.get(field)?.as_ref()
    }

    fn is_list(&self, field: usize) -> bool {
        self.choice(field).is_some_and(|c| c.list.is_some())
    }

    /// The state at the start of field `field`'s value, after its literal.
    fn content(&self, field: usize) -> State {
        let fresh = State::lead(field, 0);
        match self.choice(field) {
            Some(c) if c.list.is_some() => State {
                at: At::ItemNext,
                ..fresh
            },
            Some(c) => State {
                at: At::Text,
                alive: all_words(c.words.len()),
                ..fresh
            },
            None => State {
                at: At::Text,
                ..fresh
            },
        }
    }

    pub(crate) fn start(&self) -> State {
        State::lead(0, 0)
    }

    /// The literal before field `field`, or the end past the last: a list's
    /// closing `]` stands where a string's closing quote would.
    fn lead_parts(&self, field: usize) -> (&'static [u8], &'static [u8], &'static [u8]) {
        let after_list = field > 0 && self.is_list(field - 1);
        match self.fields.get(field) {
            None if after_list => (b"]}", b"", b""),
            None => (b"\"}", b"", b""),
            Some(f) => {
                let open: &'static [u8] = match (field, after_list) {
                    (0, _) => b"{\"",
                    (_, true) => b"],\"",
                    (_, false) => b"\",\"",
                };
                let close: &'static [u8] = match self.is_list(field) {
                    true => b"\":[",
                    false => b"\":\"",
                };
                (open, f.name.as_bytes(), close)
            }
        }
    }

    fn lead_len(&self, field: usize) -> usize {
        let (open, name, close) = self.lead_parts(field);
        open.len() + name.len() + close.len()
    }

    fn lead_byte(&self, field: usize, at: usize) -> Option<u8> {
        let (open, name, close) = self.lead_parts(field);
        open.iter().chain(name).chain(close).nth(at).copied()
    }

    /// The state after `byte`, or why it cannot come. Past a hard field's limit
    /// a byte is masked rather than ending the answer.
    pub(crate) fn step(&self, state: State, byte: u8) -> Result<State, Stop> {
        let hard = self.fields.get(state.field).is_some_and(|f| f.hard);
        match self.step_soft(state, byte) {
            Err(Stop::Over(_)) if hard => Err(Stop::Masked),
            other => other,
        }
    }

    fn step_soft(&self, state: State, byte: u8) -> Result<State, Stop> {
        let n = self.fields.len();
        match state.at {
            At::Done => Err(Stop::Masked),
            At::Lead(at) => {
                if self.lead_byte(state.field, at) != Some(byte) {
                    return Err(Stop::Masked);
                }
                Ok(match at + 1 == self.lead_len(state.field) {
                    false => State {
                        at: At::Lead(at + 1),
                        ..state
                    },
                    true if state.field == n => State {
                        at: At::Done,
                        ..state
                    },
                    true => self.content(state.field),
                })
            }
            At::Text => match self.choice(state.field) {
                Some(choice) => self.choose(choice, state, byte),
                None => self.text(state, byte),
            },
            At::Escape => match byte {
                b'"' | b'\\' | b'/' => self.character(state),
                b'n' => Ok(self.blank(state, true)),
                _ => Err(Stop::Masked),
            },
            At::ItemNext | At::ItemOpen | At::Item | At::ItemDone => self.item(state, byte),
        }
    }

    /// A byte in a text field's string.
    fn text(&self, state: State, byte: u8) -> Result<State, Stop> {
        match byte {
            b'"' => Ok(State::lead(state.field + 1, 1)),
            b'\\' => Ok(State {
                at: At::Escape,
                ..state
            }),
            b' ' => Ok(self.blank(state, false)),
            b'!'..=b'~' => self.character(state),
            _ => Err(Stop::Masked),
        }
    }

    /// A byte in a choice's word: the closing quote once a word is complete, or
    /// a byte that keeps it the start of a word.
    fn choose(&self, choice: &Choice<'_>, state: State, byte: u8) -> Result<State, Stop> {
        let ends = |(i, w): (usize, &&str)| state.alive & (1 << i) != 0 && w.len() == state.chars;
        if byte == b'"'
            && let Some((word, _)) = choice.words.iter().enumerate().find(|&w| ends(w))
        {
            return Ok(match state.at {
                At::Item => State {
                    at: At::ItemDone,
                    items: state.items + 1,
                    used: state.used | 1 << word,
                    ..state
                },
                _ => State::lead(state.field + 1, 1),
            });
        }
        let begins = |(i, w): &(usize, &&str)| {
            state.alive & (1 << i) != 0 && w.as_bytes().get(state.chars) == Some(&byte)
        };
        let alive = choice
            .words
            .iter()
            .enumerate()
            .filter(begins)
            .fold(0, |mask, (i, _)| mask | 1 << i);
        match alive {
            0 => Err(Stop::Masked),
            alive => Ok(State {
                alive,
                chars: state.chars + 1,
                ..state
            }),
        }
    }

    /// A byte of a list: an item's quotes and word, the commas between, and the
    /// closing `]`, which starts the literal after the list.
    fn item(&self, state: State, byte: u8) -> Result<State, Stop> {
        let Some(choice) = self.choice(state.field) else {
            return Err(Stop::Masked);
        };
        // A word appears once in a list; the next item may take any other.
        let unused = all_words(choice.words.len()) & !state.used;
        let more = state.items < choice.list.unwrap_or(0) && unused != 0;
        let open = State {
            at: At::Item,
            chars: 0,
            alive: unused,
            ..state
        };
        match (state.at, byte) {
            (At::ItemNext, b'"') if more => Ok(open),
            (At::ItemOpen, b'"') => Ok(open),
            (At::ItemNext | At::ItemDone, b']') => Ok(State::lead(state.field + 1, 1)),
            (At::ItemDone, b',') if more => Ok(State {
                at: At::ItemOpen,
                ..state
            }),
            (At::Item, _) => self.choose(choice, state, byte),
            _ => Err(Stop::Masked),
        }
    }

    /// One more character other than a space or a newline: past a limit, or
    /// after spaces or newlines that went past one, the string is over.
    fn character(&self, state: State) -> Result<State, Stop> {
        if let Some(overflow) = state.tail {
            return Err(Stop::Over(overflow));
        }
        match self.room(&state) {
            true => Ok(State {
                at: At::Text,
                chars: state.chars + 1,
                line_chars: state.line_chars + 1,
                seen: true,
                ..state
            }),
            false => Err(Stop::Over(self.over(&state, false))),
        }
    }

    /// A space, or with `newline` a newline. The reader trims them where they
    /// end the string (and where they start it in a field that trims its start),
    /// so past a limit they only mark the string's tail.
    fn blank(&self, state: State, newline: bool) -> State {
        let state = State {
            at: At::Text,
            ..state
        };
        let skipped = self.fields[state.field].trim_start && !state.seen;
        if skipped || state.tail.is_some() {
            return state;
        }
        match (newline, self.room(&state), self.newline_room(&state)) {
            (false, true, _) => State {
                chars: state.chars + 1,
                line_chars: state.line_chars + 1,
                ..state
            },
            (true, _, true) => State {
                chars: state.chars + 1,
                lines: state.lines + 1,
                line_chars: 0,
                ..state
            },
            _ => State {
                tail: Some(self.over(&state, newline)),
                ..state
            },
        }
    }

    fn room(&self, s: &State) -> bool {
        let f = &self.fields[s.field];
        s.chars < f.max_chars && s.line_chars < f.max_line_chars
    }

    fn newline_room(&self, s: &State) -> bool {
        let f = &self.fields[s.field];
        s.lines + 1 < f.max_lines && s.chars < f.max_chars
    }

    /// The limit a character (or, with `newline`, a line) would go past.
    fn over(&self, s: &State, newline: bool) -> Overflow {
        let limit = match (s.chars >= self.fields[s.field].max_chars, newline) {
            (true, _) => Limit::Chars,
            (false, true) => Limit::Lines,
            (false, false) => Limit::LineChars,
        };
        Overflow {
            field: s.field,
            limit,
            line: s.lines + 1,
        }
    }

    /// The state after all of `bytes`, or why the first one that cannot come
    /// cannot.
    pub(crate) fn advance(&self, state: State, bytes: &[u8]) -> Result<State, Stop> {
        bytes
            .iter()
            .try_fold(state, |state, &byte| self.step(state, byte))
    }

    /// Fills `out` with the bytes the shape alone decides after `state`: the rest
    /// of the literal it is in (`","summary":"`), or none outside a literal.
    pub(crate) fn forced(&self, state: &State, out: &mut Vec<u8>) {
        out.clear();
        if let At::Lead(at) = state.at {
            let (open, name, close) = self.lead_parts(state.field);
            out.extend(open.iter().chain(name).chain(close).skip(at));
        }
    }

    /// The fewest bytes that close the object from `state`: the rest of the
    /// literal, an escape's last byte, the quotes and the literals after it.
    pub(crate) fn need(&self, state: &State) -> usize {
        let k = state.field;
        let after = || self.lead_len(k + 1) + self.suffix[k + 1];
        // The fewest bytes that finish a word the bytes so far begin.
        let rest = |mask: u64, chars: usize| {
            self.choice(k).map_or(0, |c| {
                c.words
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, w)| w.len().saturating_sub(chars))
                    .min()
                    .unwrap_or(0)
            })
        };
        match state.at {
            At::Done => 0,
            At::Lead(at) => self.lead_len(k) - at + self.suffix[k],
            At::Text => rest(state.alive, state.chars) + after(),
            At::Escape => 1 + after(),
            At::ItemNext | At::ItemDone => after(),
            At::ItemOpen => 2 + rest(!state.used, 0) + after(),
            At::Item => rest(state.alive, state.chars) + 1 + after(),
        }
    }

    /// Fills `out` with the ids of `tokens` that may come after `state`: those
    /// whose bytes fit the shape, past a limit included. Control and user-defined
    /// tokens are never in `tokens`, so no end token comes: the answer ends when
    /// the object closes, at a limit, cut off, or when the caller stops it.
    pub(crate) fn allowed(
        &self,
        state: &State,
        tok: &Tokenizer,
        tokens: &[u32],
        out: &mut Vec<u32>,
    ) {
        out.clear();
        out.extend(
            tokens.iter().copied().filter(|&id| {
                !matches!(self.advance(*state, tok.token_bytes(id)), Err(Stop::Masked))
            }),
        );
    }
}

/// The plain tokens a shape can ever allow: those made only of printable ASCII,
/// in rising order.
pub(crate) fn ascii_tokens(tok: &Tokenizer) -> Result<Vec<u32>, Error> {
    let mut ids = Vec::new();
    for id in 0..tok.len() as u32 {
        let bytes = tok.token_bytes(id);
        let plain = matches!(tok.kind(id), Kind::Normal | Kind::Byte);
        if plain && !bytes.is_empty() && bytes.iter().all(|b| (b' '..=b'~').contains(b)) {
            ids.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Whether every printable ASCII byte has a plain token of its own among
/// `ascii` (from [`ascii_tokens`]). bpe refuses a vocabulary without its 256
/// byte tokens, so this holds today; the engine checks it anyway rather than
/// rely on that.
pub(crate) fn spells_ascii(tok: &Tokenizer, ascii: &[u32]) -> bool {
    let mut found = [false; 95];
    for &id in ascii {
        if let [byte] = tok.token_bytes(id) {
            found[usize::from(byte - b' ')] = true;
        }
    }
    found.iter().all(|&f| f)
}
