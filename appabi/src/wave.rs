//! WAVE, the WebAssembly Value Encoding (ADR 0003 section 5): how calls, replies,
//! the action log and `show` write `appabi` values as text. Typed both ways: a
//! value prints by its type, and text parses against an expected type.
//!
//! Covered: `true`/`false`; integers; floats (`nan`, `inf`, `-inf`); `'c'`;
//! `"string"` with `\n \t \r \\ \" \' \u{..}` escapes; lists `[a, b]`; tuples
//! `(a, b)`; records `{name: v, ...}` in field order (an `option` field that is
//! `none` may be left out); enum cases `name`; variants `name` or `name(v)`;
//! `some(v)`/`none`; `ok`/`ok(v)`/`err`/`err(v)`; flags `{a, b}`. Names are
//! kebab-case. Handles have no text form.

use crate::{Ty, Val};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

/// The longest text a value may be parsed from.
pub const MAX_TEXT: usize = 64 << 10;
/// The deepest nesting parsed.
const MAX_DEPTH: usize = 32;

/// `value` as WAVE text, by `ty`.
pub fn print(value: &Val, ty: &Ty) -> Result<String, String> {
    let mut out = String::new();
    write(&mut out, value, ty)?;
    Ok(out)
}

fn mismatch(value: &Val, ty: &Ty) -> String {
    format!("{value:?} is not a {ty:?}")
}

fn write(out: &mut String, value: &Val, ty: &Ty) -> Result<(), String> {
    match (value, ty) {
        (Val::Bool(b), Ty::Bool) => out.push_str(if *b { "true" } else { "false" }),
        (Val::U8(v), Ty::U8) => push(out, v),
        (Val::U16(v), Ty::U16) => push(out, v),
        (Val::U32(v), Ty::U32) => push(out, v),
        (Val::U64(v), Ty::U64) => push(out, v),
        (Val::S8(v), Ty::S8) => push(out, v),
        (Val::S16(v), Ty::S16) => push(out, v),
        (Val::S32(v), Ty::S32) => push(out, v),
        (Val::S64(v), Ty::S64) => push(out, v),
        (Val::F32(v), Ty::F32) => float(out, f64::from(*v)),
        (Val::F64(v), Ty::F64) => float(out, *v),
        (Val::Char(c), Ty::Char) => {
            out.push('\'');
            escape(out, *c, '\'');
            out.push('\'');
        }
        (Val::String(s), Ty::String) => {
            out.push('"');
            for c in s.chars() {
                escape(out, c, '"');
            }
            out.push('"');
        }
        (Val::Bytes(bytes), Ty::List(item)) if **item == Ty::U8 => {
            let items: Vec<Val> = bytes.iter().map(|b| Val::U8(*b)).collect();
            write(out, &Val::List(items), ty)?;
        }
        (Val::List(items), Ty::List(item)) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(out, v, item)?;
            }
            out.push(']');
        }
        (Val::Tuple(items), Ty::Tuple(types)) if items.len() == types.len() => {
            out.push('(');
            for (i, (v, t)) in items.iter().zip(types).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(out, v, t)?;
            }
            out.push(')');
        }
        (Val::Record(fields), Ty::Record(types)) if fields.len() == types.len() => {
            out.push('{');
            for (i, (v, (name, t))) in fields.iter().zip(types).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
                out.push_str(": ");
                write(out, v, t)?;
            }
            out.push('}');
        }
        (Val::Enum(case), Ty::Enum(names)) => {
            out.push_str(
                names
                    .get(*case as usize)
                    .ok_or_else(|| mismatch(value, ty))?,
            );
        }
        (Val::Variant(case, payload), Ty::Variant(cases)) => {
            let (name, payload_ty) = cases
                .get(*case as usize)
                .ok_or_else(|| mismatch(value, ty))?;
            out.push_str(name);
            match (payload, payload_ty) {
                (Some(v), Some(t)) => {
                    out.push('(');
                    write(out, v, t)?;
                    out.push(')');
                }
                (None, None) => {}
                _ => return Err(mismatch(value, ty)),
            }
        }
        (Val::Option(inner), Ty::Option(t)) => match inner {
            Some(v) => {
                out.push_str("some(");
                write(out, v, t)?;
                out.push(')');
            }
            None => out.push_str("none"),
        },
        (Val::Result(result), Ty::Result(ok, err)) => {
            let (name, payload, payload_ty) = match result {
                Ok(v) => ("ok", v, ok),
                Err(v) => ("err", v, err),
            };
            out.push_str(name);
            match (payload, payload_ty) {
                (Some(v), Some(t)) => {
                    out.push('(');
                    write(out, v, t)?;
                    out.push(')');
                }
                (None, None) => {}
                _ => return Err(mismatch(value, ty)),
            }
        }
        (Val::Flags(set), Ty::Flags(names)) => {
            out.push('{');
            for (i, flag) in set.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(
                    names
                        .get(*flag as usize)
                        .ok_or_else(|| mismatch(value, ty))?,
                );
            }
            out.push('}');
        }
        _ => return Err(mismatch(value, ty)),
    }
    Ok(())
}

fn push(out: &mut String, v: impl core::fmt::Display) {
    let _ = write!(out, "{v}");
}

fn float(out: &mut String, v: f64) {
    if v.is_nan() {
        out.push_str("nan");
    } else if v.is_infinite() {
        out.push_str(if v > 0.0 { "inf" } else { "-inf" });
    } else {
        let _ = write!(out, "{v:?}");
    }
}

fn escape(out: &mut String, c: char, quote: char) {
    match c {
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\\' => out.push_str("\\\\"),
        c if c == quote => {
            out.push('\\');
            out.push(c);
        }
        c if c.is_control() => {
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        }
        c => out.push(c),
    }
}

/// Parses `text` as one value of `ty`; nothing but spaces may follow it.
pub fn parse(text: &str, ty: &Ty) -> Result<Val, String> {
    if text.len() > MAX_TEXT {
        return Err("the value text is too long".into());
    }
    let mut p = Parser { text, at: 0 };
    let value = p.value(ty, 0)?;
    p.space();
    if p.at != text.len() {
        return Err(format!("unexpected text after the value at byte {}", p.at));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn space(&mut self) {
        let skipped = self.rest().len() - self.rest().trim_start().len();
        self.at += skipped;
    }

    fn eat(&mut self, token: &str) -> bool {
        self.space();
        if self.rest().starts_with(token) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!("expected {token:?} at byte {}", self.at))
        }
    }

    /// A kebab-case name or a number-ish token: letters, digits, `-`, `.`, `+`, `_`.
    fn word(&mut self) -> &str {
        self.space();
        let rest = self.rest();
        let len = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+' | '_')))
            .map_or(rest.len(), |(i, _)| i);
        let start = self.at;
        self.at += len;
        &self.text[start..start + len]
    }

    fn number<T: core::str::FromStr>(&mut self) -> Result<T, String> {
        let word = self.word();
        word.parse()
            .map_err(|_| format!("{word:?} is not a number of that type"))
    }

    fn quoted(&mut self, quote: char) -> Result<String, String> {
        self.space();
        if !self.rest().starts_with(quote) {
            return Err(format!("expected {quote} at byte {}", self.at));
        }
        self.at += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        loop {
            let (i, c) = chars.next().ok_or("an unterminated quote")?;
            match c {
                c if c == quote => {
                    self.at += i + 1;
                    return Ok(out);
                }
                '\\' => {
                    let (_, e) = chars.next().ok_or("an unterminated escape")?;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '\\' | '"' | '\'' => out.push(e),
                        'u' => {
                            if chars.next().map(|(_, c)| c) != Some('{') {
                                return Err("expected { after \\u".into());
                            }
                            let mut hex = String::new();
                            loop {
                                let (_, h) = chars.next().ok_or("an unterminated \\u{}")?;
                                if h == '}' {
                                    break;
                                }
                                if hex.len() >= 6 {
                                    return Err("a \\u{} escape is too long".into());
                                }
                                hex.push(h);
                            }
                            let code =
                                u32::from_str_radix(&hex, 16).map_err(|_| "a bad \\u{} escape")?;
                            out.push(char::from_u32(code).ok_or("\\u{} names no character")?);
                        }
                        other => return Err(format!("an unknown escape \\{other}")),
                    }
                }
                c => out.push(c),
            }
        }
    }

    /// A comma-separated sequence up to `close`, parsing each item with `item`.
    fn items(
        &mut self,
        close: &str,
        mut item: impl FnMut(&mut Self, usize) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut index = 0;
        if self.eat(close) {
            return Ok(());
        }
        loop {
            item(self, index)?;
            index += 1;
            if self.eat(close) {
                return Ok(());
            }
            self.expect(",")?;
            if self.eat(close) {
                return Ok(());
            }
        }
    }

    fn payload(&mut self, ty: &Option<Box<Ty>>, depth: usize) -> Result<Option<Box<Val>>, String> {
        match ty {
            Some(t) => {
                self.expect("(")?;
                let v = self.value(t, depth)?;
                self.expect(")")?;
                Ok(Some(Box::new(v)))
            }
            None => Ok(None),
        }
    }

    fn value(&mut self, ty: &Ty, depth: usize) -> Result<Val, String> {
        if depth > MAX_DEPTH {
            return Err("the value nests too deeply".into());
        }
        let depth = depth + 1;
        Ok(match ty {
            Ty::Bool => match self.word() {
                "true" => Val::Bool(true),
                "false" => Val::Bool(false),
                other => return Err(format!("{other:?} is not a bool")),
            },
            Ty::U8 => Val::U8(self.number()?),
            Ty::U16 => Val::U16(self.number()?),
            Ty::U32 => Val::U32(self.number()?),
            Ty::U64 => Val::U64(self.number()?),
            Ty::S8 => Val::S8(self.number()?),
            Ty::S16 => Val::S16(self.number()?),
            Ty::S32 => Val::S32(self.number()?),
            Ty::S64 => Val::S64(self.number()?),
            Ty::F32 | Ty::F64 => {
                let word = self.word();
                let v = match word {
                    "nan" => f64::NAN,
                    "inf" => f64::INFINITY,
                    "-inf" => f64::NEG_INFINITY,
                    w => w.parse().map_err(|_| format!("{w:?} is not a float"))?,
                };
                if matches!(ty, Ty::F32) {
                    Val::F32(v as f32)
                } else {
                    Val::F64(v)
                }
            }
            Ty::Char => {
                let s = self.quoted('\'')?;
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Val::Char(c),
                    _ => return Err("a char holds one character".into()),
                }
            }
            Ty::String => Val::String(self.quoted('"')?),
            Ty::List(item) => {
                self.expect("[")?;
                let mut items = Vec::new();
                self.items("]", |p, _| {
                    items.push(p.value(item, depth)?);
                    Ok(())
                })?;
                Val::List(items)
            }
            Ty::Tuple(types) => {
                self.expect("(")?;
                let mut items = Vec::new();
                self.items(")", |p, i| {
                    let t = types.get(i).ok_or("too many tuple items")?;
                    items.push(p.value(t, depth)?);
                    Ok(())
                })?;
                if items.len() != types.len() {
                    return Err("too few tuple items".into());
                }
                Val::Tuple(items)
            }
            Ty::Record(fields) => {
                self.expect("{")?;
                let mut values: Vec<Option<Val>> = fields.iter().map(|_| None).collect();
                self.items("}", |p, _| {
                    let name = p.word().to_string();
                    let i = fields
                        .iter()
                        .position(|(n, _)| *n == name)
                        .ok_or_else(|| format!("no field {name}"))?;
                    if values[i].is_some() {
                        return Err(format!("field {name} given twice"));
                    }
                    p.expect(":")?;
                    values[i] = Some(p.value(&fields[i].1, depth)?);
                    Ok(())
                })?;
                let mut out = Vec::with_capacity(fields.len());
                for (v, (name, t)) in values.into_iter().zip(fields) {
                    out.push(match (v, t) {
                        (Some(v), _) => v,
                        (None, Ty::Option(_)) => Val::Option(None),
                        (None, _) => return Err(format!("field {name} is missing")),
                    });
                }
                Val::Record(out)
            }
            Ty::Enum(names) => {
                let word = self.word();
                let i = names
                    .iter()
                    .position(|n| n == word)
                    .ok_or_else(|| format!("no case {word}"))?;
                Val::Enum(i as u32)
            }
            Ty::Variant(cases) => {
                let word = self.word().to_string();
                let i = cases
                    .iter()
                    .position(|(n, _)| *n == word)
                    .ok_or_else(|| format!("no case {word}"))?;
                let payload = self.payload(&cases[i].1.clone().map(Box::new), depth)?;
                Val::Variant(i as u32, payload)
            }
            Ty::Option(t) => match self.word() {
                "none" => Val::Option(None),
                "some" => {
                    self.expect("(")?;
                    let v = self.value(t, depth)?;
                    self.expect(")")?;
                    Val::Option(Some(Box::new(v)))
                }
                other => return Err(format!("{other:?} is not some(..) or none")),
            },
            Ty::Result(ok, err) => match self.word() {
                "ok" => Val::Result(Ok(self.payload(ok, depth)?)),
                "err" => Val::Result(Err(self.payload(err, depth)?)),
                other => return Err(format!("{other:?} is not ok or err")),
            },
            Ty::Flags(names) => {
                self.expect("{")?;
                let mut set = Vec::new();
                self.items("}", |p, _| {
                    let word = p.word();
                    let i = names
                        .iter()
                        .position(|n| n == word)
                        .ok_or_else(|| format!("no flag {word}"))?;
                    if !set.contains(&(i as u32)) {
                        set.push(i as u32);
                    }
                    Ok(())
                })?;
                set.sort_unstable();
                Val::Flags(set)
            }
            Ty::Own | Ty::Borrow => return Err("a handle has no text form".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn s(text: &str) -> String {
        String::from(text)
    }

    #[test]
    fn values_round_trip() {
        let rect = Ty::Record(vec![
            (s("x"), Ty::U32),
            (s("label"), Ty::Option(Box::new(Ty::String))),
        ]);
        let kind = Ty::Enum(vec![s("root"), s("row")]);
        let shape = Ty::Variant(vec![(s("circle"), Some(Ty::F64)), (s("empty"), None)]);
        let cases: Vec<(Ty, Val, &str)> = vec![
            (Ty::Bool, Val::Bool(true), "true"),
            (Ty::S32, Val::S32(-7), "-7"),
            (Ty::U64, Val::U64(u64::MAX), "18446744073709551615"),
            (Ty::F64, Val::F64(1.5), "1.5"),
            (Ty::F64, Val::F64(f64::INFINITY), "inf"),
            (Ty::Char, Val::Char('\''), r"'\''"),
            (Ty::String, Val::String(s("a \"b\"\n")), r#""a \"b\"\n""#),
            (
                Ty::List(Box::new(Ty::U8)),
                Val::List(vec![Val::U8(1), Val::U8(2)]),
                "[1, 2]",
            ),
            (
                Ty::Tuple(vec![Ty::U8, Ty::Bool]),
                Val::Tuple(vec![Val::U8(1), Val::Bool(false)]),
                "(1, false)",
            ),
            (
                rect.clone(),
                Val::Record(vec![Val::U32(3), Val::Option(None)]),
                "{x: 3, label: none}",
            ),
            (kind.clone(), Val::Enum(1), "row"),
            (
                shape.clone(),
                Val::Variant(0, Some(Box::new(Val::F64(2.0)))),
                "circle(2.0)",
            ),
            (shape, Val::Variant(1, None), "empty"),
            (
                Ty::Result(Some(Box::new(Ty::U32)), Some(Box::new(Ty::String))),
                Val::Result(Err(Some(Box::new(Val::String(s("no")))))),
                r#"err("no")"#,
            ),
            (Ty::Result(None, None), Val::Result(Ok(None)), "ok"),
            (
                Ty::Flags(vec![s("read"), s("write")]),
                Val::Flags(vec![0, 1]),
                "{read, write}",
            ),
        ];
        for (ty, value, text) in cases {
            assert_eq!(print(&value, &ty).unwrap(), text);
            assert_eq!(parse(text, &ty).unwrap(), value, "{text}");
        }
        // A missing option field is none; spaces and a trailing comma are fine.
        assert_eq!(
            parse(" { x : 3 , } ", &rect).unwrap(),
            Val::Record(vec![Val::U32(3), Val::Option(None)])
        );
        assert_eq!(
            parse(r#""\u{1F600}""#, &Ty::String).unwrap(),
            Val::String(s("\u{1F600}"))
        );
    }

    #[test]
    fn bad_text_is_refused() {
        let rect = Ty::Record(vec![(s("x"), Ty::U32)]);
        for (text, ty) in [
            ("300", Ty::U8),
            ("-1", Ty::U32),
            ("1 2", Ty::U32),
            ("{}", rect.clone()),
            ("{x: 1, x: 2}", rect.clone()),
            ("{y: 1}", rect),
            ("\"open", Ty::String),
            ("'ab'", Ty::Char),
            ("maybe", Ty::Bool),
            ("[1, 2", Ty::List(Box::new(Ty::U8))),
            (r#""\q""#, Ty::String),
            (r#""\u{110000}""#, Ty::String),
        ] {
            assert!(parse(text, &ty).is_err(), "{text}");
        }
        let deep = "[".repeat(40) + &"]".repeat(40);
        let mut ty = Ty::U8;
        for _ in 0..40 {
            ty = Ty::List(Box::new(ty));
        }
        assert!(parse(&deep, &ty).is_err());
    }
}
