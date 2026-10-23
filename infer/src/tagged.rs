//! The tagged call (ADR 0002 D4): every answer is
//! `{"name":"<action>","arguments":{...}}` with no spaces. `name` is a choice over
//! any number of actions (past [`crate::json::MAX_WORDS`]); once it closes,
//! `arguments` takes that action's fields in declared order, each steered by its
//! [`Kind`]: an enum word, a bounded `Int`, a string within its limits, a host
//! name, a `List` of scalars or an `Opt` (`null`). Limits steer hard: a byte past
//! one is masked, so a closed answer always parses and fits, and a `\` that could
//! not finish an escape never comes.
//!
//! It is a [`Constraint`], so the engine decodes under it with jump-forward over
//! every byte only one continuation allows (`","arguments":{"name":"` after
//! `set_policy`, the rest of a name or a word once its first bytes pick it), and
//! token healing: the last forced token is the model's to pick, so it may take
//! a longer one (`"/` after `"`). Spike S02-1 measured this decoder, as
//! actbench's prototype, at 57-58 of 60 exact calls on Qwen3-4B-2507.

use crate::Error;
use crate::json::{Constraint, Stop};
use alloc::vec::Vec;

/// An argument's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    /// One of these words: printable ASCII without quotes or backslashes.
    Enum(&'a [&'a str]),
    /// A whole number in `min..=max`, `min` at least 0.
    Int { min: i64, max: i64 },
    /// A string of at most `chars` characters in at most `lines` lines (a
    /// newline is written `\n`) of at most `line_chars`; `empty` allows "".
    Text {
        chars: u16,
        lines: u8,
        line_chars: u16,
        empty: bool,
    },
    /// A host name: lowercase letters, digits, `.` and `-`.
    Host,
    /// At most this many scalars.
    List(&'a Kind<'a>, u8),
    /// The value or `null`; the value is a scalar or a list.
    Opt(&'a Kind<'a>),
}

/// An argument: a plain name and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arg<'a> {
    pub name: &'a str,
    pub kind: Kind<'a>,
}

/// An action an answer may name, with its arguments in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action<'a> {
    pub name: &'a str,
    pub args: &'a [Arg<'a>],
}

/// Longest host name.
const HOST_MAX: usize = 253;
/// Longest action or enum word the state's byte counts hold.
const MAX_WORD: usize = 254;

fn plain(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn scalar_kind(kind: &Kind<'_>) -> bool {
    match kind {
        Kind::Enum(words) => {
            !words.is_empty()
                && words.len() <= usize::from(u16::MAX)
                && words.iter().all(|w| {
                    !w.is_empty()
                        && w.len() <= MAX_WORD
                        && w.bytes()
                            .all(|b| matches!(b, b' '..=b'~') && b != b'"' && b != b'\\')
                })
                && (1..words.len()).all(|j| !words[..j].contains(&words[j]))
        }
        Kind::Int { min, max } => 0 <= *min && min <= max,
        Kind::Text {
            chars,
            lines,
            line_chars,
            ..
        } => *chars > 0 && *lines > 0 && *line_chars > 0,
        Kind::Host => true,
        Kind::List(..) | Kind::Opt(_) => false,
    }
}

fn valid_kind(kind: &Kind<'_>) -> bool {
    match kind {
        Kind::List(inner, _) => scalar_kind(inner),
        Kind::Opt(inner) => {
            scalar_kind(inner) || matches!(inner, Kind::List(item, _) if scalar_kind(item))
        }
        other => scalar_kind(other),
    }
}

/// Whether `actions` can be decoded: at least one, plain distinct names (an
/// argument's within its action too) and kinds the state can hold.
pub fn check(actions: &[Action<'_>]) -> Result<(), Error> {
    if actions.is_empty() || actions.len() > usize::from(u16::MAX) {
        return Err(Error::Shape("has no actions, or too many"));
    }
    for (i, action) in actions.iter().enumerate() {
        if !plain(action.name) || action.name.len() > MAX_WORD {
            return Err(Error::Shape("has an action name that is not a plain word"));
        }
        if actions[..i].iter().any(|a| a.name == action.name) {
            return Err(Error::Shape("has two actions of one name"));
        }
        if action.args.len() > 64 {
            return Err(Error::Shape("has an action with too many arguments"));
        }
        for (j, arg) in action.args.iter().enumerate() {
            if !plain(arg.name) || action.args[..j].iter().any(|a| a.name == arg.name) {
                return Err(Error::Shape(
                    "has an argument name that is not a plain, distinct word",
                ));
            }
            if !valid_kind(&arg.kind) {
                return Err(Error::Shape("has an argument kind it cannot steer"));
            }
        }
    }
    Ok(())
}

const OPEN: &[u8] = b"{\"name\":\"";
const NULL: &[u8] = b"null";
/// Most bytes one jump-forward takes.
const MAX_FORCED: usize = 256;

/// One part of a row's answer after its name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg<'a> {
    Lit(Vec<u8>),
    Val(Kind<'a>),
}

/// A scalar value under way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scalar {
    /// 0 before it, 1 in it, 2 closed (strings only; an int ends at the byte
    /// after it).
    stage: u8,
    escape: bool,
    chars: u16,
    lines: u8,
    line_chars: u16,
    /// An enum: the lowest word the bytes so far begin, and how many bytes.
    cand: u16,
    len: u8,
    num: i64,
    digits: u8,
}

/// A value under way: a scalar, or a list or an option around one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Value {
    /// A list: 0 before `[`, 1 after `[`, 2 in an item, 3 after an item, 4
    /// after `,`, 5 closed.
    list: u8,
    items: u8,
    /// Bytes of `null` so far.
    null: u8,
    scalar: Scalar,
}

/// Where an answer is: the opening literal, the name, or an action's
/// arguments (the segment, the byte in a literal, the value under way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Open(u8),
    Name {
        cand: u16,
        len: u8,
    },
    Row {
        row: u16,
        seg: u8,
        at: u16,
        value: Value,
    },
}

/// The tagged grammar over a set of actions.
pub struct Tagged<'a> {
    names: Vec<&'a str>,
    segs: Vec<Vec<Seg<'a>>>,
    /// `tail[r][k]`: the fewest bytes from the start of action r's segment k to
    /// the end.
    tail: Vec<Vec<usize>>,
}

/// The parts after an action's name.
fn segments<'a>(action: &Action<'a>) -> Vec<Seg<'a>> {
    let mut segs = Vec::new();
    let mut lit = b"\",\"arguments\":{".to_vec();
    for (i, arg) in action.args.iter().enumerate() {
        if i > 0 {
            lit.push(b',');
        }
        lit.push(b'"');
        lit.extend_from_slice(arg.name.as_bytes());
        lit.extend_from_slice(b"\":");
        segs.push(Seg::Lit(core::mem::take(&mut lit)));
        segs.push(Seg::Val(arg.kind));
    }
    lit.extend_from_slice(b"}}");
    segs.push(Seg::Lit(lit));
    segs
}

/// Decimal digits of `n`, at least 0.
fn digits(n: i64) -> usize {
    let mut n = n.max(0);
    let mut count = 1;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}
/// The fewest bytes of a whole value.
fn shortest(ty: &Kind<'_>) -> usize {
    match ty {
        Kind::Enum(words) => 2 + words.iter().map(|w| w.len()).min().unwrap_or(0),
        Kind::Int { min, .. } => digits(*min),
        Kind::Text { .. } | Kind::Host => 2,
        Kind::List(..) => 2,
        Kind::Opt(inner) => shortest(inner).min(NULL.len()),
    }
}

/// Whether some integer in `min..=max` has the decimal digits `num` (of
/// `count` digits, no leading zero) as its start.
fn int_prefix(num: i64, count: u8, min: i64, max: i64) -> bool {
    if count > 1 && num < 10i64.pow(u32::from(count) - 1) {
        return false;
    }
    let mut low = num;
    let mut span = 1i64;
    for _ in 0..19 {
        if low > max {
            return false;
        }
        if low + span > min && !(num == 0 && span > 1) {
            return true;
        }
        match (low.checked_mul(10), span.checked_mul(10)) {
            (Some(l), Some(s)) => (low, span) = (l, s),
            _ => return false,
        }
    }
    false
}

/// What a scalar does with a byte.
enum Took {
    Into(Scalar),
    /// It was complete before the byte, which belongs to what follows.
    Ended,
}

/// String limits: (characters, lines, characters a line).
fn limits(ty: &Kind<'_>) -> (usize, usize, usize) {
    match ty {
        Kind::Text {
            chars,
            lines,
            line_chars,
            ..
        } => (
            usize::from(*chars),
            usize::from(*lines),
            usize::from(*line_chars),
        ),
        Kind::Host => (HOST_MAX, 1, HOST_MAX),
        _ => (0, 0, 0),
    }
}

/// A string that may hold `\n`.
fn multiline(ty: &Kind<'_>) -> bool {
    matches!(ty, Kind::Text { lines, .. } if *lines > 1)
}

/// A string that may be empty.
fn may_be_empty(ty: &Kind<'_>) -> bool {
    matches!(ty, Kind::Text { empty: true, .. })
}

fn host_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'
}

/// One character into a string within its limits; a newline is `\n`.
fn character(ty: &Kind<'_>, mut s: Scalar, newline: bool) -> Result<Took, Stop> {
    let (chars, lines, line_chars) = limits(ty);
    if usize::from(s.chars) + 1 > chars {
        return Err(Stop::Masked);
    }
    if newline {
        if usize::from(s.lines) + 2 > lines {
            return Err(Stop::Masked);
        }
        s.lines += 1;
        s.line_chars = 0;
    } else {
        if usize::from(s.line_chars) + 1 > line_chars {
            return Err(Stop::Masked);
        }
        s.line_chars += 1;
    }
    s.chars += 1;
    Ok(Took::Into(s))
}

/// Whether an escape can still finish: room for one more character.
fn escapable(ty: &Kind<'_>, s: &Scalar) -> bool {
    character(ty, *s, false).is_ok() || (multiline(ty) && character(ty, *s, true).is_ok())
}

fn scalar(ty: &Kind<'_>, s: Scalar, b: u8) -> Result<Took, Stop> {
    match ty {
        Kind::Int { min, max } => {
            if b.is_ascii_digit() {
                let num = s.num * 10 + i64::from(b - b'0');
                let count = s.digits + 1;
                return match count <= 19 && int_prefix(num, count, *min, *max) {
                    true => Ok(Took::Into(Scalar {
                        stage: 1,
                        num,
                        digits: count,
                        ..s
                    })),
                    false => Err(Stop::Masked),
                };
            }
            match s.digits > 0 && (*min..=*max).contains(&s.num) {
                true => Ok(Took::Ended),
                false => Err(Stop::Masked),
            }
        }
        _ if s.stage == 2 => Ok(Took::Ended),
        _ if s.stage == 0 => match b {
            b'"' => Ok(Took::Into(Scalar { stage: 1, ..s })),
            _ => Err(Stop::Masked),
        },
        Kind::Enum(words) => {
            let prefix = &words[usize::from(s.cand)].as_bytes()[..usize::from(s.len)];
            if b == b'"' {
                return match words.iter().any(|w| w.as_bytes() == prefix) {
                    true => Ok(Took::Into(Scalar { stage: 2, ..s })),
                    false => Err(Stop::Masked),
                };
            }
            let next = words.iter().position(|w| {
                let w = w.as_bytes();
                w.len() > prefix.len() && w.starts_with(prefix) && w[prefix.len()] == b
            });
            match next {
                Some(cand) => Ok(Took::Into(Scalar {
                    cand: cand as u16,
                    len: s.len + 1,
                    ..s
                })),
                None => Err(Stop::Masked),
            }
        }
        _ if s.escape => match b {
            b'"' | b'\\' | b'/' => character(ty, Scalar { escape: false, ..s }, false),
            b'n' if multiline(ty) => character(ty, Scalar { escape: false, ..s }, true),
            _ => Err(Stop::Masked),
        },
        _ => match b {
            b'"' if s.chars > 0 || may_be_empty(ty) => Ok(Took::Into(Scalar { stage: 2, ..s })),
            b'"' => Err(Stop::Masked),
            // Only with room for the character it makes, or it is a dead end.
            b'\\' if !matches!(ty, Kind::Host) && escapable(ty, &s) => {
                Ok(Took::Into(Scalar { escape: true, ..s }))
            }
            b'\\' => Err(Stop::Masked),
            _ if matches!(ty, Kind::Host) && !host_byte(b) => Err(Stop::Masked),
            b' '..=b'~' if b != b'\\' => character(ty, s, false),
            _ => Err(Stop::Masked),
        },
    }
}

/// Whether a scalar is whole and closed (an int never is: the next byte ends it).
fn closed(ty: &Kind<'_>, s: &Scalar) -> bool {
    !matches!(ty, Kind::Int { .. }) && s.stage == 2
}

/// The fewest bytes that finish a scalar under way.
fn scalar_need(ty: &Kind<'_>, s: &Scalar) -> usize {
    match ty {
        Kind::Int { min, max } => match s.digits {
            0 => digits(*min),
            _ if (*min..=*max).contains(&s.num) => 0,
            _ => 1,
        },
        _ if s.stage == 0 => shortest(ty),
        _ if s.stage == 2 => 0,
        Kind::Enum(words) => {
            let prefix = &words[usize::from(s.cand)].as_bytes()[..usize::from(s.len)];
            let rest = words
                .iter()
                .filter(|w| w.as_bytes().starts_with(prefix))
                .map(|w| w.len() - prefix.len())
                .min()
                .unwrap_or(0);
            rest + 1
        }
        _ if s.escape => 2,
        _ => 1,
    }
}

/// A value's byte: `Some(value)` taken, `None` when the value was complete
/// before it.
fn value(ty: &Kind<'_>, v: Value, b: u8) -> Result<Option<Value>, Stop> {
    match ty {
        Kind::Opt(inner) => {
            if v.null > 0 || (v.scalar == Scalar::default() && v.list == 0 && b == b'n') {
                let at = usize::from(v.null);
                return match (at, NULL.get(at)) {
                    (4, _) => Ok(None),
                    (_, Some(&want)) if want == b => Ok(Some(Value {
                        null: v.null + 1,
                        ..v
                    })),
                    _ => Err(Stop::Masked),
                };
            }
            value(inner, v, b)
        }
        Kind::List(inner, max) => match (v.list, b) {
            (0, b'[') => Ok(Some(Value { list: 1, ..v })),
            (1 | 3, b']') => Ok(Some(Value { list: 5, ..v })),
            (3, b',') if v.items < *max => Ok(Some(Value { list: 4, ..v })),
            (1 | 4 | 2, _) => {
                let s = if v.list == 2 {
                    v.scalar
                } else {
                    Scalar::default()
                };
                match scalar(inner, s, b)? {
                    Took::Into(s) => Ok(Some(Value {
                        list: if closed(inner, &s) { 3 } else { 2 },
                        items: if v.list == 2 { v.items } else { v.items + 1 },
                        scalar: s,
                        ..v
                    })),
                    // An int item ended: the byte is `,` or `]`.
                    Took::Ended => value(ty, Value { list: 3, ..v }, b),
                }
            }
            (5, _) => Ok(None),
            _ => Err(Stop::Masked),
        },
        _ => match scalar(ty, v.scalar, b)? {
            Took::Into(s) => Ok(Some(Value { scalar: s, ..v })),
            Took::Ended => Ok(None),
        },
    }
}

/// Whether a value is whole and closed, so the next segment starts.
fn value_closed(ty: &Kind<'_>, v: &Value) -> bool {
    match ty {
        Kind::Opt(inner) => usize::from(v.null) == NULL.len() || value_closed(inner, v),
        Kind::List(..) => v.list == 5,
        _ => closed(ty, &v.scalar),
    }
}

fn value_need(ty: &Kind<'_>, v: &Value) -> usize {
    match ty {
        Kind::Opt(_) if v.null > 0 => NULL.len() - usize::from(v.null),
        Kind::Opt(inner) if *v == Value::default() => shortest(ty).min(shortest(inner)),
        Kind::Opt(inner) => value_need(inner, v),
        Kind::List(inner, _) => match v.list {
            0 => 2,
            1 | 3 => 1,
            2 => scalar_need(inner, &v.scalar) + 1,
            4 => shortest(inner) + 1,
            _ => 0,
        },
        _ => scalar_need(ty, &v.scalar),
    }
}

impl<'a> Tagged<'a> {
    /// The grammar over `actions`, after [`check`].
    pub fn new(actions: &[Action<'a>]) -> Result<Self, Error> {
        check(actions)?;
        let names: Vec<&'a str> = actions.iter().map(|a| a.name).collect();
        let segs: Vec<Vec<Seg<'a>>> = actions.iter().map(segments).collect();
        let tail = segs
            .iter()
            .map(|segs| {
                let mut tail = alloc::vec![0; segs.len() + 1];
                for k in (0..segs.len()).rev() {
                    tail[k] = tail[k + 1]
                        + match &segs[k] {
                            Seg::Lit(lit) => lit.len(),
                            Seg::Val(ty) => shortest(ty),
                        };
                }
                tail
            })
            .collect();
        Ok(Self { names, segs, tail })
    }

    /// The action an answer in `state` names, once its name has closed.
    pub fn action(&self, state: &State) -> Option<&'a str> {
        match state {
            State::Row { row, .. } => Some(self.names[usize::from(*row)]),
            _ => None,
        }
    }

    /// The row a name of these bytes is, if one is exactly that.
    fn exact(&self, prefix: &[u8]) -> Option<usize> {
        self.names.iter().position(|n| n.as_bytes() == prefix)
    }

    fn prefix(&self, cand: u16, len: u8) -> &[u8] {
        &self.names[usize::from(cand)].as_bytes()[..usize::from(len)]
    }

    /// Row `row` at segment `seg`, its start.
    fn at(row: usize, seg: usize) -> State {
        State::Row {
            row: row as u16,
            seg: seg as u8,
            at: 0,
            value: Value::default(),
        }
    }

    fn in_row(&self, row: usize, seg: usize, at: usize, v: Value, b: u8) -> Result<State, Stop> {
        match self.segs[row].get(seg) {
            None => Err(Stop::Masked),
            Some(Seg::Lit(lit)) if lit[at] == b => match at + 1 == lit.len() {
                true => Ok(Self::at(row, seg + 1)),
                false => Ok(State::Row {
                    row: row as u16,
                    seg: seg as u8,
                    at: (at + 1) as u16,
                    value: v,
                }),
            },
            Some(Seg::Lit(_)) => Err(Stop::Masked),
            Some(Seg::Val(ty)) => match value(ty, v, b)? {
                Some(v) if value_closed(ty, &v) => Ok(Self::at(row, seg + 1)),
                Some(v) => Ok(State::Row {
                    row: row as u16,
                    seg: seg as u8,
                    at: 0,
                    value: v,
                }),
                None => self.in_row(row, seg + 1, 0, Value::default(), b),
            },
        }
    }
}

impl Constraint for Tagged<'_> {
    type State = State;

    fn start(&self) -> State {
        State::Open(0)
    }

    fn step(&self, state: State, b: u8) -> Result<State, Stop> {
        match state {
            State::Open(at) => match OPEN.get(usize::from(at)) {
                Some(&want) if want == b && usize::from(at) + 1 == OPEN.len() => {
                    Ok(State::Name { cand: 0, len: 0 })
                }
                Some(&want) if want == b => Ok(State::Open(at + 1)),
                _ => Err(Stop::Masked),
            },
            State::Name { cand, len } => {
                let prefix = self.prefix(cand, len);
                if b == b'"' {
                    return self
                        .exact(prefix)
                        .map(|row| self.in_row(row, 0, 0, Value::default(), b))
                        .unwrap_or(Err(Stop::Masked));
                }
                let next = self.names.iter().position(|n| {
                    let n = n.as_bytes();
                    n.len() > prefix.len() && n.starts_with(prefix) && n[prefix.len()] == b
                });
                match next {
                    Some(cand) => Ok(State::Name {
                        cand: cand as u16,
                        len: len + 1,
                    }),
                    None => Err(Stop::Masked),
                }
            }
            State::Row {
                row,
                seg,
                at,
                value,
            } => self.in_row(
                usize::from(row),
                usize::from(seg),
                usize::from(at),
                value,
                b,
            ),
        }
    }

    fn is_done(&self, state: &State) -> bool {
        matches!(state, State::Row { row, seg, .. }
            if usize::from(*seg) == self.segs[usize::from(*row)].len())
    }

    /// Every byte that is the only one allowed, in a row.
    fn forced(&self, state: &State, out: &mut Vec<u8>) {
        out.clear();
        let mut state = *state;
        while out.len() < MAX_FORCED && !self.is_done(&state) {
            let mut only = None;
            for b in b' '..=b'~' {
                if let Ok(next) = self.step(state, b) {
                    if only.is_some() {
                        return;
                    }
                    only = Some((b, next));
                }
            }
            let Some((b, next)) = only else { return };
            out.push(b);
            state = next;
        }
    }

    fn heals(&self) -> bool {
        true
    }

    fn need(&self, state: &State) -> usize {
        // From a whole name to the end: its closing quote leads segment 0.
        let finish = |row: usize| self.tail[row][0];
        match *state {
            State::Open(at) => {
                let best = (0..self.names.len())
                    .map(|r| self.names[r].len() + finish(r))
                    .min()
                    .unwrap_or(0);
                OPEN.len() - usize::from(at) + best
            }
            State::Name { cand, len } => {
                let prefix = self.prefix(cand, len);
                (0..self.names.len())
                    .filter(|&r| self.names[r].as_bytes().starts_with(prefix))
                    .map(|r| self.names[r].len() - prefix.len() + finish(r))
                    .min()
                    .unwrap_or(0)
            }
            State::Row {
                row,
                seg,
                at,
                value,
            } => {
                let (row, seg) = (usize::from(row), usize::from(seg));
                let rest = match self.segs[row].get(seg) {
                    None => return 0,
                    Some(Seg::Lit(lit)) => lit.len() - usize::from(at),
                    Some(Seg::Val(ty)) => value_need(ty, &value),
                };
                rest + self.tail[row][seg + 1]
            }
        }
    }
}

#[cfg(test)]
mod tests;
