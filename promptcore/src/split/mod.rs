//! Splits an entry's tokens into top-level items (ADR 0012 D1): directives,
//! declarations, function definitions and statements, each with its byte span so
//! its text is copied verbatim.
//!
//! This is not a C parser. It finds where each item ends (a `;` or block at
//! bracket depth 0, following `if`, `else`, `do` and the loops) and reads only
//! what the prelude needs from declarations: their storage class, their
//! specifiers, any `struct`, `union` or `enum` body they define, and each
//! declarator's name and initializer. The compiler judges everything else.
//!
//! An item starts a declaration when its first word is a declaration keyword or
//! a known type name, or when two names begin it (`Point p;`, with `Point` a
//! type `cc` has not seen). Otherwise it is a statement.

mod decl;

use crate::Error;
use crate::lex::{Kind, Token};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Most statements nested in one another, so a hostile entry cannot exhaust the
/// stack.
pub const MAX_NESTING: usize = 64;

/// Why a directive inside brackets is refused.
const DIRECTIVE_INSIDE: &str = "a directive inside brackets; put it at the top of the entry";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Byte span in the entry, from the first token to the last.
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub what: What,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// `#define NAME …`.
    Define(String),
    /// `#undef NAME`.
    Undef(String),
    /// `#include <header>`, the header with its brackets.
    Include(String),
    Decl(Decl),
    Func(Func),
    /// A statement, and whether it ended with `;` (or a block).
    Stmt {
        form: Form,
        semi: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    /// A block, or a statement that starts with a keyword: `if`, `for`, `return`.
    Control,
    Expr,
    /// One name alone, as `Name;` (convenience 4).
    Lone(String),
    /// Starts with a string literal, as `"x=%d\n", x;` (convenience 3).
    Str,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    #[default]
    None,
    Typedef,
    Extern,
    Static,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decl {
    pub storage: Storage,
    /// A `const` anywhere in it, so its initializer stays a static one.
    pub constant: bool,
    /// The specifiers without the storage class, words joined by spaces, with
    /// any tagged body cut to its tag: `const struct Point`.
    pub specifiers: String,
    /// The tagged bodies it defines.
    pub tags: Vec<Tag>,
    /// The specifiers keep an untagged body, which cannot be repeated per
    /// declarator: `struct { int x; } a, b;`.
    pub anonymous: bool,
    /// The constants of untagged `enum` bodies.
    pub enumerators: Vec<String>,
    pub declarators: Vec<Declarator>,
    pub semi: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// `struct`, `union` or `enum`.
    pub kind: String,
    pub name: String,
    /// The definition, as written, with a `;`: `struct Point { int x, y; };`.
    pub text: String,
    /// The constants an `enum` body defines.
    pub enumerators: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declarator {
    pub name: String,
    /// As written, without its initializer: `*names[4]`.
    pub text: String,
    pub init: Option<String>,
    /// A function declarator: the name is followed by its parameters.
    pub function: bool,
    /// It has an array part, so it cannot be assigned.
    pub array: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    pub name: String,
    /// The declaration before the body, with a `;`: `I64 Sq(I64 x);`.
    pub proto: String,
    /// `static`: the entry's own, never in the prelude.
    pub local: bool,
    /// Takes no arguments: `()` or `(void)`.
    pub nullary: bool,
}

/// Words that start a declaration.
const DECL_WORDS: [&str; 31] = [
    "typedef",
    "extern",
    "static",
    "auto",
    "register",
    "_Thread_local",
    "const",
    "volatile",
    "restrict",
    "_Atomic",
    "inline",
    "_Noreturn",
    "void",
    "char",
    "short",
    "int",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    "_Bool",
    "_Complex",
    "struct",
    "union",
    "enum",
    "__attribute__",
    "_Alignas",
    "__inline",
    "__restrict",
    "thread_local",
];

/// Words that start a statement that is not an expression.
const CONTROL_WORDS: [&str; 12] = [
    "if",
    "while",
    "for",
    "do",
    "switch",
    "return",
    "break",
    "continue",
    "goto",
    "case",
    "default",
    "_Static_assert",
];

/// The rest of C's keywords: never a name.
const OTHER_KEYWORDS: [&str; 5] = ["else", "sizeof", "_Alignof", "_Generic", "typeof"];

pub fn keyword(word: &str) -> bool {
    DECL_WORDS.contains(&word) || CONTROL_WORDS.contains(&word) || OTHER_KEYWORDS.contains(&word)
}

/// Words a declarator may hold besides its name.
fn declarator_word(word: &str) -> bool {
    matches!(
        word,
        "const" | "volatile" | "restrict" | "__restrict" | "_Atomic"
    )
}

/// The items of an entry; `is_type` says whether a name is a type the session
/// knows (kai.h's and its typedefs).
pub fn split(
    src: &str,
    tokens: &[Token],
    is_type: &dyn Fn(&str) -> bool,
) -> Result<Vec<Item>, Error> {
    let mut splitter = Splitter {
        src,
        t: tokens,
        pos: 0,
        is_type,
        local_types: Vec::new(),
    };
    splitter.items()
}

struct Splitter<'a> {
    src: &'a str,
    t: &'a [Token],
    pos: usize,
    is_type: &'a dyn Fn(&str) -> bool,
    /// Typedef names this entry defines, for its later items.
    local_types: Vec<String>,
}

fn syntax(line: u32, what: &'static str) -> Error {
    Error::Syntax { line, what }
}

impl<'a> Splitter<'a> {
    fn items(&mut self) -> Result<Vec<Item>, Error> {
        let mut items: Vec<Item> = Vec::new();
        while let Some(tok) = self.t.get(self.pos).copied() {
            let first = self.pos;
            let what = match (tok.kind, tok.text(self.src)) {
                (Kind::Directive, text) => {
                    self.pos += 1;
                    match directive(text, tok.line)? {
                        Some(what) => what,
                        None => continue,
                    }
                }
                (Kind::Punct, ";") => {
                    self.pos += 1;
                    continue;
                }
                _ if self.starts_decl() => self.declaration(first)?,
                _ => self.statement_item(first)?,
            };
            if let Some(last) = items.last()
                && !finished(&last.what)
            {
                return Err(syntax(last.line, "a ; is missing at the end of this"));
            }
            let end = self.t[self.pos - 1].end;
            items.push(Item {
                start: tok.start,
                end,
                line: tok.line,
                what,
            });
        }
        Ok(items)
    }

    fn text(&self, i: usize) -> &'a str {
        self.t[i].text(self.src)
    }

    fn punct_at(&self, i: usize) -> Option<&'a str> {
        self.t
            .get(i)
            .filter(|t| t.kind == Kind::Punct)
            .map(|t| t.text(self.src))
    }

    fn word_at(&self, i: usize) -> Option<&'a str> {
        self.t
            .get(i)
            .filter(|t| t.kind == Kind::Ident)
            .map(|t| t.text(self.src))
    }

    fn last_line(&self) -> u32 {
        self.t.last().map_or(1, |t| t.line)
    }

    fn known_type(&self, word: &str) -> bool {
        (self.is_type)(word) || self.local_types.iter().any(|t| t == word)
    }

    fn starts_decl(&self) -> bool {
        let Some(word) = self.word_at(self.pos) else {
            return false;
        };
        if DECL_WORDS.contains(&word) || self.known_type(word) {
            return true;
        }
        !keyword(word) && self.word_at(self.pos + 1).is_some()
    }

    /// The index after the bracket that closes the one at `open`. A directive
    /// inside brackets is refused: copied into a prototype, a declarator or a
    /// body that the prelude repeats, it would change how later entries
    /// compile.
    fn group_end(&self, open: usize) -> Result<usize, Error> {
        let mut stack: Vec<&str> = Vec::new();
        for i in open..self.t.len() {
            if self.t[i].kind == Kind::Directive {
                return Err(syntax(self.t[i].line, DIRECTIVE_INSIDE));
            }
            let Some(p) = self.punct_at(i) else {
                continue;
            };
            match p {
                "(" => stack.push(")"),
                "[" => stack.push("]"),
                "{" => stack.push("}"),
                ")" | "]" | "}" => {
                    if stack.pop() != Some(p) {
                        return Err(syntax(self.t[i].line, "the brackets do not match"));
                    }
                    if stack.is_empty() {
                        return Ok(i + 1);
                    }
                }
                _ => {}
            }
        }
        Err(syntax(self.t[open].line, "a bracket is not closed"))
    }

    fn statement_item(&mut self, first: usize) -> Result<What, Error> {
        let tok = self.t[first];
        let form = match (tok.kind, tok.text(self.src)) {
            (Kind::Punct, "{") => Form::Control,
            (Kind::Ident, word) if CONTROL_WORDS.contains(&word) => Form::Control,
            (Kind::Str, _) => Form::Str,
            _ => Form::Expr,
        };
        let semi = self.statement(0)?;
        let used = self.pos - first - usize::from(semi && form != Form::Control);
        let form = match form {
            Form::Expr if used == 1 && tok.kind == Kind::Ident => {
                Form::Lone(tok.text(self.src).to_string())
            }
            form => form,
        };
        Ok(What::Stmt { form, semi })
    }

    /// Skips one statement; true if it ended with `;` or a block.
    fn statement(&mut self, depth: usize) -> Result<bool, Error> {
        if depth > MAX_NESTING {
            return Err(syntax(self.last_line(), "statements nest too deeply"));
        }
        let Some(tok) = self.t.get(self.pos).copied() else {
            return Err(syntax(self.last_line(), "a statement is missing"));
        };
        match (tok.kind, tok.text(self.src)) {
            (Kind::Punct, "{") => {
                self.pos = self.group_end(self.pos)?;
                Ok(true)
            }
            (Kind::Ident, "if") => {
                self.pos += 1;
                self.parens(tok.line)?;
                let done = self.statement(depth + 1)?;
                if self.word_at(self.pos) == Some("else") {
                    self.pos += 1;
                    return self.statement(depth + 1);
                }
                Ok(done)
            }
            (Kind::Ident, "while" | "for" | "switch") => {
                self.pos += 1;
                self.parens(tok.line)?;
                self.statement(depth + 1)
            }
            (Kind::Ident, "do") => {
                self.pos += 1;
                self.statement(depth + 1)?;
                if self.word_at(self.pos) != Some("while") {
                    return Err(syntax(tok.line, "do needs its while"));
                }
                self.pos += 1;
                self.parens(tok.line)?;
                self.simple()
            }
            (Kind::Ident, "else") => Err(syntax(tok.line, "else without if")),
            (Kind::Directive, _) => Err(syntax(tok.line, "a statement is missing")),
            _ => self.simple(),
        }
    }

    fn parens(&mut self, line: u32) -> Result<(), Error> {
        if self.punct_at(self.pos) != Some("(") {
            return Err(syntax(line, "( is missing"));
        }
        self.pos = self.group_end(self.pos)?;
        Ok(())
    }

    /// Skips to the `;` at depth 0; false if the entry or a directive came first.
    fn simple(&mut self) -> Result<bool, Error> {
        while let Some(tok) = self.t.get(self.pos).copied() {
            match (tok.kind, tok.text(self.src)) {
                (Kind::Directive, _) => return Ok(false),
                (Kind::Punct, ";") => {
                    self.pos += 1;
                    return Ok(true);
                }
                (Kind::Punct, "(" | "[" | "{") => self.pos = self.group_end(self.pos)?,
                (Kind::Punct, ")" | "]" | "}") => {
                    return Err(syntax(tok.line, "a closing bracket with nothing open"));
                }
                _ => self.pos += 1,
            }
        }
        Ok(false)
    }

    fn text_at(&self, i: usize) -> Option<&'a str> {
        self.t.get(i).map(|t| t.text(self.src))
    }
}

fn finished(what: &What) -> bool {
    match what {
        What::Stmt { semi, .. } => *semi,
        What::Decl(decl) => decl.semi,
        What::Define(_) | What::Undef(_) | What::Include(_) | What::Func(_) => true,
    }
}

/// A directive at the top of an entry: `#define`, `#undef` and `#include <…>`
/// only, because a conditional around items would hide what they define.
/// `None` for the null directive, a `#` alone.
fn directive(text: &str, line: u32) -> Result<Option<What>, Error> {
    let rest = text[1..].trim_start_matches([' ', '\t']);
    let word_len = rest.bytes().take_while(|c| c.is_ascii_alphabetic()).count();
    let (word, rest) = rest.split_at(word_len);
    let rest = rest.trim_start_matches([' ', '\t']);
    let name = || {
        let len = rest
            .bytes()
            .take_while(|&c| c.is_ascii_alphanumeric() || c == b'_')
            .count();
        match &rest[..len] {
            "" => Err(syntax(line, "the directive needs a name")),
            name if name.as_bytes()[0].is_ascii_digit() => {
                Err(syntax(line, "the directive needs a name"))
            }
            name => Ok(name.to_string()),
        }
    };
    match word {
        "" if rest.trim().is_empty() => Ok(None),
        "define" => Ok(Some(What::Define(name()?))),
        "undef" => Ok(Some(What::Undef(name()?))),
        "include" if rest.starts_with('<') => match rest.find('>') {
            Some(close) if header_name(&rest[1..close]) => {
                Ok(Some(What::Include(rest[..=close].to_string())))
            }
            Some(_) => Err(syntax(line, "not a header name")),
            None => Err(syntax(line, "#include <… needs its >")),
        },
        "include" => Err(syntax(
            line,
            "only #include <…> works at c>; files come with :load",
        )),
        _ => Err(syntax(
            line,
            "only #define, #undef and #include <…> work at the top of an entry",
        )),
    }
}

/// A header the compiler's `/kai/include` could hold: letters, digits, `_`,
/// `-`, `.` and `/`, with no empty, `.` or `..` part.
fn header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'/'))
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}
