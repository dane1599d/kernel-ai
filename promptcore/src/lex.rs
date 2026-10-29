//! A C tokenizer just deep enough to split an entry into items: identifiers
//! (keywords among them), numbers, string and character literals, punctuators,
//! and whole preprocessor lines. Comments and whitespace only separate tokens.
//! Every token keeps its byte span, so items copy the user's text verbatim.
//!
//! Outside literals and comments only ASCII is C; any other byte is refused here
//! with its line rather than passed on to the compiler.

use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An identifier or a keyword.
    Ident,
    Number,
    /// A string literal, with any `L`, `u`, `U` or `u8` prefix.
    Str,
    /// A character literal, with any prefix.
    Char,
    Punct,
    /// A whole preprocessor line, `#` to its end, continuations included.
    Directive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// Byte span in the entry.
    pub start: usize,
    pub end: usize,
    /// The line the token starts on, from 1.
    pub line: u32,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexError {
    /// A `/*` comment with no `*/`.
    OpenComment { line: u32 },
    /// A string or character literal cut off by a newline or the end.
    OpenQuote { line: u32 },
    /// A byte C does not allow outside literals and comments.
    Stray { line: u32, byte: u8 },
    /// A name longer than [`MAX_NAME`] bytes.
    LongName { line: u32 },
}

/// Longest name: C guarantees 63 significant characters, and `kai-cc 1`
/// carries names of at most this length.
pub const MAX_NAME: usize = 63;

impl LexError {
    pub fn line(&self) -> u32 {
        match *self {
            LexError::OpenComment { line }
            | LexError::OpenQuote { line }
            | LexError::Stray { line, .. }
            | LexError::LongName { line } => line,
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::OpenComment { .. } => f.write_str("a /* comment is not closed"),
            LexError::OpenQuote { .. } => f.write_str("a string or character is not closed"),
            LexError::Stray { byte, .. } if byte.is_ascii_graphic() => {
                write!(f, "'{}' is not C", *byte as char)
            }
            LexError::Stray { byte, .. } => write!(f, "byte 0x{byte:02x} is not C"),
            LexError::LongName { .. } => write!(f, "a name is longer than {MAX_NAME} bytes"),
        }
    }
}

/// C's punctuators, longest first, so the first match is the longest.
const PUNCTS: [&[u8]; 48] = [
    b"...", b"<<=", b">>=", b"->", b"++", b"--", b"<<", b">>", b"<=", b">=", b"==", b"!=", b"&&",
    b"||", b"*=", b"/=", b"%=", b"+=", b"-=", b"&=", b"^=", b"|=", b"##", b"[", b"]", b"(", b")",
    b"{", b"}", b".", b"&", b"*", b"+", b"-", b"~", b"!", b"/", b"%", b"<", b">", b"^", b"|", b"?",
    b":", b";", b"=", b",", b"#",
];

/// The tokens of `src`; at most one per byte.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let b = src.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut line = 1u32;
    // Only whitespace and comments since the last newline: a `#` starts a directive.
    let mut line_start = true;
    while let Some(&c) = b.get(at) {
        let next = b.get(at + 1).copied();
        match c {
            b'\n' => {
                line += 1;
                line_start = true;
                at += 1;
            }
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => at += 1,
            b'\\' if next == Some(b'\n') => {
                line += 1;
                at += 2;
            }
            b'/' if next == Some(b'/') => at = line_end(b, at, &mut line),
            b'/' if next == Some(b'*') => at = comment_end(b, at, &mut line)?,
            b'#' if line_start => {
                let first = line;
                let end = directive_end(b, at, &mut line);
                tokens.push(token(Kind::Directive, at, end, first));
                at = end;
                line_start = false;
            }
            _ => {
                let first = line;
                let (kind, end) = one(b, at, &mut line)?;
                tokens.push(token(kind, at, end, first));
                at = end;
                line_start = false;
            }
        }
    }
    Ok(tokens)
}

fn token(kind: Kind, start: usize, end: usize, line: u32) -> Token {
    Token {
        kind,
        start,
        end,
        line,
    }
}

/// One token that is not a directive: its kind and end.
fn one(b: &[u8], at: usize, line: &mut u32) -> Result<(Kind, usize), LexError> {
    let c = b[at];
    if c.is_ascii_alphabetic() || c == b'_' {
        let end = at + b[at..].iter().take_while(|&&c| ident_byte(c)).count();
        if end - at > MAX_NAME {
            return Err(LexError::LongName { line: *line });
        }
        // A literal's prefix: `L"…"`, `u8'…'` and the like.
        if matches!(&b[at..end], b"L" | b"u" | b"U" | b"u8") {
            match b.get(end) {
                Some(b'"') => return Ok((Kind::Str, quoted(b, end, line)?)),
                Some(b'\'') => return Ok((Kind::Char, quoted(b, end, line)?)),
                _ => {}
            }
        }
        return Ok((Kind::Ident, end));
    }
    if c.is_ascii_digit() || (c == b'.' && b.get(at + 1).is_some_and(u8::is_ascii_digit)) {
        return Ok((Kind::Number, number_end(b, at)));
    }
    match c {
        b'"' => return Ok((Kind::Str, quoted(b, at, line)?)),
        b'\'' => return Ok((Kind::Char, quoted(b, at, line)?)),
        _ => {}
    }
    match PUNCTS.iter().find(|p| b[at..].starts_with(p)) {
        Some(p) => Ok((Kind::Punct, at + p.len())),
        None => Err(LexError::Stray {
            line: *line,
            byte: c,
        }),
    }
}

fn ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// A preprocessing number: digits, letters, `_`, `.`, and a sign after an
/// exponent letter.
fn number_end(b: &[u8], mut at: usize) -> usize {
    while let Some(&c) = b.get(at) {
        let exponent =
            matches!(c, b'e' | b'E' | b'p' | b'P') && matches!(b.get(at + 1), Some(b'+' | b'-'));
        if exponent {
            at += 2;
        } else if ident_byte(c) || c == b'.' {
            at += 1;
        } else {
            break;
        }
    }
    at
}

/// The end of the literal whose opening quote is at `at`.
fn quoted(b: &[u8], at: usize, line: &mut u32) -> Result<usize, LexError> {
    let quote = b[at];
    let mut i = at + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => {
                if b.get(i + 1) == Some(&b'\n') {
                    *line += 1;
                }
                i += 2;
            }
            b'\n' => break,
            c if c == quote => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(LexError::OpenQuote { line: *line })
}

/// The end of the `//` comment at `at`: its line, and the next ones a
/// backslash splices on, as the compiler reads them.
fn line_end(b: &[u8], at: usize, line: &mut u32) -> usize {
    let mut i = at;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' if b.get(i + 1) == Some(&b'\n') => {
                *line += 1;
                i += 2;
            }
            b'\n' => return i,
            _ => i += 1,
        }
    }
    i
}

/// The end of the block comment opening at `at`.
fn comment_end(b: &[u8], at: usize, line: &mut u32) -> Result<usize, LexError> {
    let first = *line;
    let mut i = at + 2;
    while let Some(&c) = b.get(i) {
        match c {
            b'*' if b.get(i + 1) == Some(&b'/') => return Ok(i + 2),
            b'\n' => *line += 1,
            _ => {}
        }
        i += 1;
    }
    Err(LexError::OpenComment { line: first })
}

/// The end of the directive starting at `at`: the first newline that is not
/// escaped and not inside a block comment (the newline itself is not part of it).
/// A quote a newline cuts off ends there, as `#error don't` does, rather than
/// failing: the compiler judges directives.
fn directive_end(b: &[u8], at: usize, line: &mut u32) -> usize {
    let mut i = at + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\n' => return i,
            b'\\' if b.get(i + 1) == Some(&b'\n') => {
                *line += 1;
                i += 2;
            }
            b'"' | b'\'' => i = directive_quote(b, i, line),
            b'/' if b.get(i + 1) == Some(&b'/') => return line_end(b, i, line),
            b'/' if b.get(i + 1) == Some(&b'*') => {
                // An open comment runs to the end; lex() never gets past it.
                i = comment_end(b, i, line).unwrap_or(b.len());
            }
            _ => i += 1,
        }
    }
    b.len()
}

fn directive_quote(b: &[u8], at: usize, line: &mut u32) -> usize {
    let quote = b[at];
    let mut i = at + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' if b.get(i + 1) == Some(&b'\n') => {
                *line += 1;
                i += 2;
            }
            b'\\' => i += 2,
            b'\n' => return i,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len().min(i)
}

/// Whether `entry` needs another line before it can run: a bracket still open,
/// a block comment not closed, or a last line ending in a backslash. The shell
/// then replies `more` (ADR 0012 D1). A closing bracket with nothing open, an
/// open quote or a stray byte make it finished, so the compiler reports them.
pub fn open(entry: &str) -> bool {
    if entry.trim_end_matches([' ', '\t', '\r']).ends_with('\\') {
        return true;
    }
    let tokens = match lex(entry) {
        Ok(tokens) => tokens,
        Err(LexError::OpenComment { .. }) => return true,
        Err(_) => return false,
    };
    let mut depth = 0usize;
    for t in tokens.iter().filter(|t| t.kind == Kind::Punct) {
        match t.text(entry) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => match depth.checked_sub(1) {
                Some(less) => depth = less,
                None => return false,
            },
            _ => {}
        }
    }
    depth > 0
}
