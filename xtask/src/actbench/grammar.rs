//! S02-1's prototype tagged decoder (ADR 0002 D4): every answer is
//! `{"name":"<row>","arguments":{...}}` with no spaces. `name` is a choice over
//! any number of rows (past today's 64-word [`infer::json::Choice`]); once it
//! closes, `arguments` takes that row's fields in declared order, each steered
//! by its [`Ty`]: an enum word, a bounded `Int`, a string within its limits, a
//! `List` or an `Opt` (`null`). Limits steer hard: a byte past one is masked,
//! so a closed answer always parses and fits. It runs on the engine through
//! [`infer::json::Constraint`], with jump-forward over every byte only one
//! continuation allows (`","arguments":{"name":"` after `set_policy`, and the
//! rest of a name once its first bytes pick it). Task 6 moves a decoder like
//! this into `infer/src/json.rs`.

use super::registry::{Row, Ty};
use infer::json::{Constraint, Stop};

const OPEN: &[u8] = b"{\"name\":\"";
const NULL: &[u8] = b"null";
/// Most bytes one jump-forward takes.
const MAX_FORCED: usize = 256;

/// One part of a row's answer after its name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Lit(Vec<u8>),
    Val(Ty),
}

/// A scalar value under way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Scalar {
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
struct Value {
    /// A list: 0 before `[`, 1 after `[`, 2 in an item, 3 after an item, 4
    /// after `,`, 5 closed.
    list: u8,
    items: u8,
    /// Bytes of `null` so far.
    null: u8,
    scalar: Scalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(private_interfaces)]
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

/// The tagged grammar over a registry.
pub struct Tagged {
    names: Vec<&'static str>,
    segs: Vec<Vec<Seg>>,
    /// `tail[r][k]`: the fewest bytes from the start of row r's segment k to
    /// the end.
    tail: Vec<Vec<usize>>,
}

/// The parts after row `row`'s name.
fn segments(row: &Row) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut lit = b"\",\"arguments\":{".to_vec();
    for (i, arg) in row.args.iter().enumerate() {
        if i > 0 {
            lit.push(b',');
        }
        lit.extend_from_slice(format!("\"{}\":", arg.name).as_bytes());
        segs.push(Seg::Lit(std::mem::take(&mut lit)));
        segs.push(Seg::Val(arg.ty));
    }
    lit.extend_from_slice(b"}}");
    segs.push(Seg::Lit(lit));
    segs
}

fn digits(n: i64) -> usize {
    n.max(0).to_string().len()
}

/// The fewest bytes of a whole value.
fn shortest(ty: &Ty) -> usize {
    match ty {
        Ty::Enum(words) => 2 + words.iter().map(|w| w.len()).min().unwrap_or(0),
        Ty::Int { min, .. } => digits(*min),
        Ty::Text(_) | Ty::Host | Ty::Path | Ty::Code => 2,
        Ty::List(..) => 2,
        Ty::Opt(inner) => shortest(inner).min(NULL.len()),
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
fn limits(ty: &Ty) -> (usize, usize, usize) {
    match ty {
        Ty::Text(max) => (usize::from(*max), 1, usize::from(*max)),
        Ty::Host => (253, 1, 253),
        Ty::Path => (128, 1, 128),
        Ty::Code => (
            llmproto::script::SCRIPT_MAX_BYTES,
            llmproto::script::SCRIPT_MAX_LINES,
            llmproto::script::SCRIPT_MAX_LINE_CHARS,
        ),
        _ => (0, 0, 0),
    }
}

fn host_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'
}

/// One character into a string within its limits; a newline is `\n`.
fn character(ty: &Ty, mut s: Scalar, newline: bool) -> Result<Took, Stop> {
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

fn scalar(ty: &Ty, s: Scalar, b: u8) -> Result<Took, Stop> {
    match ty {
        Ty::Int { min, max } => {
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
        Ty::Enum(words) => {
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
            b'n' if *ty == Ty::Code => character(ty, Scalar { escape: false, ..s }, true),
            _ => Err(Stop::Masked),
        },
        _ => match b {
            b'"' if s.chars > 0 || *ty == Ty::Code || matches!(ty, Ty::Text(_)) => {
                Ok(Took::Into(Scalar { stage: 2, ..s }))
            }
            b'"' => Err(Stop::Masked),
            b'\\' if *ty != Ty::Host => Ok(Took::Into(Scalar { escape: true, ..s })),
            _ if *ty == Ty::Host && !host_byte(b) => Err(Stop::Masked),
            b' '..=b'~' if b != b'\\' => character(ty, s, false),
            _ => Err(Stop::Masked),
        },
    }
}

/// Whether a scalar is whole and closed (an int never is: the next byte ends it).
fn closed(ty: &Ty, s: &Scalar) -> bool {
    !matches!(ty, Ty::Int { .. }) && s.stage == 2
}

/// The fewest bytes that finish a scalar under way.
fn scalar_need(ty: &Ty, s: &Scalar) -> usize {
    match ty {
        Ty::Int { min, max } => match s.digits {
            0 => digits(*min),
            _ if (*min..=*max).contains(&s.num) => 0,
            _ => 1,
        },
        _ if s.stage == 0 => shortest(ty),
        _ if s.stage == 2 => 0,
        Ty::Enum(words) => {
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
fn value(ty: &Ty, v: Value, b: u8) -> Result<Option<Value>, Stop> {
    match ty {
        Ty::Opt(inner) => {
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
        Ty::List(inner, max) => match (v.list, b) {
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
fn value_closed(ty: &Ty, v: &Value) -> bool {
    match ty {
        Ty::Opt(inner) => usize::from(v.null) == NULL.len() || value_closed(inner, v),
        Ty::List(..) => v.list == 5,
        _ => closed(ty, &v.scalar),
    }
}

fn value_need(ty: &Ty, v: &Value) -> usize {
    match ty {
        Ty::Opt(_) if v.null > 0 => NULL.len() - usize::from(v.null),
        Ty::Opt(inner) if *v == Value::default() => shortest(ty).min(shortest(inner)),
        Ty::Opt(inner) => value_need(inner, v),
        Ty::List(inner, _) => match v.list {
            0 => 2,
            1 | 3 => 1,
            2 => scalar_need(inner, &v.scalar) + 1,
            4 => shortest(inner) + 1,
            _ => 0,
        },
        _ => scalar_need(ty, &v.scalar),
    }
}

impl Tagged {
    pub fn new(rows: &[&'static Row]) -> Self {
        let names: Vec<&'static str> = rows.iter().map(|row| row.name).collect();
        let segs: Vec<Vec<Seg>> = rows.iter().map(|row| segments(row)).collect();
        let tail = segs
            .iter()
            .map(|segs| {
                let mut tail = vec![0; segs.len() + 1];
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
        Self { names, segs, tail }
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

impl Constraint for Tagged {
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
mod tests {
    use super::super::registry::rows;
    use super::*;

    fn takes(g: &Tagged, text: &str) -> Result<State, Stop> {
        g.advance(g.start(), text.as_bytes())
    }

    fn whole(g: &Tagged, text: &str) -> bool {
        takes(g, text).is_ok_and(|s| g.is_done(&s))
    }

    #[test]
    fn calls_of_every_shape_parse_and_close() {
        let g = Tagged::new(&rows(80));
        for text in [
            r#"{"name":"cpus","arguments":{}}"#,
            r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
            r#"{"name":"undo","arguments":{"steps":3}}"#,
            r#"{"name":"editor_open","arguments":{"path":"/home/a.rs","line":null}}"#,
            r#"{"name":"editor_open","arguments":{"path":"/home/a.rs","line":120}}"#,
            r#"{"name":"clarify","arguments":{"question":"Which?","options":["a","b c"]}}"#,
            r#"{"name":"clarify","arguments":{"question":"Which?","options":[]}}"#,
            r#"{"name":"script","arguments":{"code":"let a = 1;\nprint(\"a\" + a);"}}"#,
            r#"{"name":"net_ping","arguments":{"host":"example.com"}}"#,
            r#"{"name":"sound_set","arguments":{"level":100}}"#,
            r#"{"name":"say","arguments":{"text":""}}"#,
        ] {
            assert!(whole(&g, text), "{text}");
            let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
            assert!(parsed["name"].is_string());
        }
    }

    #[test]
    fn the_mask_refuses_what_the_row_cannot_take() {
        let g = Tagged::new(&rows(40));
        for text in [
            r#"{"name":"cpu"#.to_owned() + "x",
            r#"{"name":"set_policy","arguments":{"name":"fifo"#.to_owned(),
            r#"{"name":"undo","arguments":{"steps":10"#.to_owned(),
            r#"{"name":"undo","arguments":{"steps":0"#.to_owned(),
            r#"{"name":"undo","arguments":{"steps":-"#.to_owned(),
            r#"{"name":"net_ping","arguments":{"host":"Example"#.to_owned(),
            r#"{"name":"say","arguments":{"text":"a\n"#.to_owned(),
            r#"{"name":"sound_set""#.to_owned(),
            r#"{"name":"cpus","arguments":{} }"#.to_owned(),
        ] {
            assert_eq!(takes(&g, &text), Err(Stop::Masked), "{text}");
        }
        let long = format!(
            r#"{{"name":"say","arguments":{{"text":"{}"#,
            "a".repeat(241)
        );
        assert_eq!(takes(&g, &long), Err(Stop::Masked));
    }

    #[test]
    fn a_list_takes_at_most_its_items() {
        let g = Tagged::new(&rows(40));
        let six =
            r#"{"name":"clarify","arguments":{"question":"q","options":["1","2","3","4","5","6""#;
        assert!(takes(&g, &format!("{six}]}}}}")).is_ok_and(|s| g.is_done(&s)));
        assert_eq!(takes(&g, &format!("{six},")), Err(Stop::Masked));
    }

    #[test]
    fn jump_forward_takes_the_literals_and_a_name_once_it_is_picked() {
        let g = Tagged::new(&rows(40));
        let mut out = Vec::new();
        g.forced(&g.start(), &mut out);
        assert_eq!(out, OPEN);
        let s = takes(&g, r#"{"name":"set_p"#).unwrap();
        g.forced(&s, &mut out);
        assert_eq!(out, br#"olicy","arguments":{"name":""#);
        let s = takes(&g, r#"{"name":"set_policy","arguments":{"name":"r"#).unwrap();
        g.forced(&s, &mut out);
        assert_eq!(out, br#"ound-robin"}}"#);
        assert!(g.is_done(&g.advance(s, &out).unwrap()));
        // `cpus` closes with no argument to pick.
        let s = takes(&g, r#"{"name":"cpus""#).unwrap();
        g.forced(&s, &mut out);
        assert_eq!(out, br#","arguments":{}}"#);
        // Inside a string the model has a choice.
        let s = takes(&g, r#"{"name":"say","arguments":{"text":""#).unwrap();
        g.forced(&s, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn need_counts_the_bytes_that_close_it() {
        let g = Tagged::new(&rows(40));
        let s = takes(&g, r#"{"name":"say","arguments":{"text":"hi"#).unwrap();
        assert_eq!(g.need(&s), 3);
        let s = takes(&g, r#"{"name":"undo","arguments":{"steps":"#).unwrap();
        assert_eq!(g.need(&s), 3);
        // The shortest whole answer from the start: a row with no arguments.
        let shortest = r#"{"name":"cpus","arguments":{}}"#.len();
        assert!(g.need(&g.start()) <= shortest);
        // From any prefix of a whole answer, need is at most the bytes left.
        let text = r#"{"name":"editor_open","arguments":{"path":"/a","line":12}}"#;
        for cut in 0..text.len() {
            let s = takes(&g, &text[..cut]).unwrap();
            assert!(g.need(&s) <= text.len() - cut, "{}", &text[..cut]);
        }
    }

    #[test]
    fn int_prefixes_follow_the_bounds() {
        assert!(int_prefix(1, 1, 1, 9));
        assert!(!int_prefix(0, 1, 1, 9));
        assert!(int_prefix(1, 1, 10, 99));
        assert!(int_prefix(0, 1, 0, 100));
        assert!(!int_prefix(10, 2, 0, 9));
        assert!(int_prefix(10, 2, 0, 100));
        assert!(!int_prefix(101, 3, 0, 100));
    }
}
