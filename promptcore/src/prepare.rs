//! One entry as a compile unit (ADR 0012 D3, step 2):
//!
//! ```text
//! #include <kai.h>
//! <prelude: every live definition the entry does not redefine>
//! <the entry's definitions, in its order>
//! void __kai_run_<seq>(void) {
//! <the entry's statements, in its order>
//! }
//! ```
//!
//! The conveniences of D1 are applied on the way: statements outside a function
//! run at once (1); a last expression with no `;` shows its value through kai.h's
//! `_Generic` macro (2); a statement that starts with a string literal is a
//! `Print` (3); `Name;` calls the session function `Name` (4). A global whose
//! initializer is a plain value (not `const`, not an array, not `{…}`) gets it
//! as an assignment in the run function, in source order, so `I64 y = Sq(3);`
//! works as at TempleOS's prompt.
//!
//! A last expression may be a call of a `void` function, which `_Generic`
//! refuses, so a [`Prepared`] that shows a value also carries a fallback unit
//! with the plain statement, compiled when the first fails.

use crate::defs::{Def, Defs, Kind, MAX_DEFS};
use crate::lex::lex;
use crate::split::{Decl, Declarator, Form, Func, Item, Storage, What, split};
use crate::{Error, MAX_ENTRY, PRINT, RUN_PREFIX, SHOW};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Type names kai.h and the C headers in `/kai/include` define, so a line that
/// starts with one is read as a declaration.
pub const BASE_TYPES: [&str; 31] = [
    "U0",
    "I8",
    "I16",
    "I32",
    "I64",
    "U8",
    "U16",
    "U32",
    "U64",
    "F64",
    "Bool",
    "size_t",
    "ssize_t",
    "ptrdiff_t",
    "intptr_t",
    "uintptr_t",
    "intmax_t",
    "uintmax_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "wchar_t",
    "va_list",
    "FILE",
    "bool",
    "off_t",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Compile, link and run.
    Eval,
    /// Definitions only: a restarted `cc` replays the shell's kept entries, and
    /// runs no statement again (D4). Globals keep only static initializers.
    Replay,
}

/// Lines of the unit that came from the entry: `(unit line, entry line, count)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineMap(Vec<(u32, u32, u32)>);

impl LineMap {
    /// The entry line of unit line `line`; `None` for the prelude and wrappers.
    pub fn entry_line(&self, line: u32) -> Option<u32> {
        self.0
            .iter()
            .find(|&&(unit, _, count)| line >= unit && line - unit < count)
            .map(|&(unit, entry, _)| entry + (line - unit))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub seq: u64,
    /// The unit to compile.
    pub source: String,
    /// The same unit with the last expression as a plain statement, for when
    /// `source` does not compile.
    pub fallback: Option<String>,
    /// The function to run, if the entry has statements.
    pub run: Option<String>,
    pub lines: LineMap,
    /// What the entry defines, to commit once it ran.
    pub defs: Vec<Def>,
    /// The macros it undefines.
    pub drops: Vec<String>,
}

/// The unit as it is written: text, the next line's number, and the line map.
#[derive(Clone)]
struct Unit {
    text: String,
    line: u32,
    map: LineMap,
}

impl Unit {
    fn raw(&mut self, text: &str) {
        self.text.push_str(text);
        self.line += newlines(text);
    }

    /// Entry text from `entry_line`, on lines of its own.
    fn chunk(&mut self, entry_line: u32, text: &str) {
        let count = newlines(text) + 1;
        self.map.0.push((self.line, entry_line, count));
        self.text.push_str(text);
        self.text.push('\n');
        self.line += count;
    }
}

fn newlines(text: &str) -> u32 {
    text.bytes().filter(|&b| b == b'\n').count() as u32
}

/// What an entry contributes, gathered item by item.
#[derive(Default)]
struct Parts {
    /// Definitions, as (entry line, text).
    section: Vec<(u32, String)>,
    /// Statements, as (entry line, text); the last may have a fallback.
    statements: Vec<(u32, String)>,
    fallback: Option<String>,
    defs: Vec<Def>,
    drops: Vec<String>,
    /// Names only this entry may use: its `static` ones.
    hidden: Vec<String>,
}

impl Defs {
    /// Entry `seq` as a unit to compile, without changing the session.
    pub fn prepare(&self, seq: u64, entry: &str, mode: Mode) -> Result<Prepared, Error> {
        if entry.len() > MAX_ENTRY {
            return Err(Error::TooLong);
        }
        let tokens = lex(entry).map_err(Error::Lex)?;
        // A last line ending in a backslash would splice the next line of a
        // later unit, the prelude's or the run function's, onto this entry's.
        let body = entry.trim_end_matches([' ', '\t', '\r', '\n']);
        if body.ends_with('\\') {
            let line = 1 + body.bytes().filter(|&b| b == b'\n').count() as u32;
            let what = "the entry ends in a backslash";
            return Err(Error::Syntax { line, what });
        }
        let is_type = |name: &str| BASE_TYPES.contains(&name) || self.is_type(name);
        let items = split(entry, &tokens, &is_type)?;
        let local_funcs: Vec<&str> = items
            .iter()
            .filter_map(|item| match &item.what {
                What::Func(f) => Some(f.name.as_str()),
                _ => None,
            })
            .collect();
        let callable = |name: &str| local_funcs.contains(&name) || self.func(name).is_some();
        let mut parts = Parts::default();
        for (i, item) in items.iter().enumerate() {
            let last = i + 1 == items.len();
            parts.item(seq, entry, item, mode, last, &callable);
        }
        dedupe(&mut parts.defs);
        if self.len() + parts.defs.len() > MAX_DEFS {
            return Err(Error::Full);
        }
        Ok(self.compose(seq, parts, mode))
    }

    fn compose(&self, seq: u64, parts: Parts, mode: Mode) -> Prepared {
        let mut except = parts.hidden;
        except.extend(parts.drops.iter().cloned());
        for def in &parts.defs {
            except.extend(def.keys.iter().cloned());
        }
        let mut unit = Unit {
            text: String::new(),
            line: 1,
            map: LineMap::default(),
        };
        unit.raw("#include <kai.h>\n");
        unit.raw(&self.prelude(&except));
        for (line, text) in &parts.section {
            unit.chunk(*line, text);
        }
        let statements = match mode {
            Mode::Eval => parts.statements,
            Mode::Replay => Vec::new(),
        };
        let Some(((last_line, last), rest)) = statements.split_last() else {
            return Prepared {
                seq,
                source: unit.text,
                fallback: None,
                run: None,
                lines: unit.map,
                defs: parts.defs,
                drops: parts.drops,
            };
        };
        let run = format!("{RUN_PREFIX}{seq}");
        unit.raw(&format!("void {run}(void) {{\n"));
        for (line, text) in rest {
            unit.chunk(*line, text);
        }
        let mut plain = parts.fallback.as_ref().map(|_| unit.clone());
        unit.chunk(*last_line, last);
        unit.raw("}\n");
        if let (Some(plain), Some(text)) = (plain.as_mut(), parts.fallback.as_ref()) {
            plain.chunk(*last_line, text);
            plain.raw("}\n");
        }
        Prepared {
            seq,
            source: unit.text,
            fallback: plain.map(|p| p.text),
            run: Some(run),
            lines: unit.map,
            defs: parts.defs,
            drops: parts.drops,
        }
    }
}

/// Keeps only the last of definitions an entry makes twice (`int x; int x = 1;`).
fn dedupe(defs: &mut Vec<Def>) {
    let mut i = 0;
    while i < defs.len() {
        let later = defs[i + 1..]
            .iter()
            .any(|d| d.keys.iter().any(|k| defs[i].keys.contains(k)));
        match later {
            true => {
                defs.remove(i);
            }
            false => i += 1,
        }
    }
}

fn def(seq: u64, kind: Kind, keys: Vec<String>, name: String, text: String) -> Def {
    Def {
        keys,
        kind,
        name,
        text,
        nullary: false,
        seq,
    }
}

impl Parts {
    fn item(
        &mut self,
        seq: u64,
        entry: &str,
        item: &Item,
        mode: Mode,
        last: bool,
        callable: &dyn Fn(&str) -> bool,
    ) {
        let text = &entry[item.start..item.end];
        let line = item.line;
        match &item.what {
            What::Define(name) => {
                let mut prelude = text.to_string();
                prelude.push('\n');
                let key = format!("#{name}");
                self.defs.push(def(
                    seq,
                    Kind::Macro,
                    alloc::vec![key],
                    format!("#define {name}"),
                    prelude,
                ));
                self.section.push((line, text.to_string()));
            }
            What::Undef(name) => {
                self.drops.push(format!("#{name}"));
                self.section.push((line, text.to_string()));
            }
            What::Include(header) => {
                let text = format!("#include {header}");
                let prelude = format!("{text}\n");
                self.defs.push(def(
                    seq,
                    Kind::Include,
                    alloc::vec![header.clone()],
                    header.clone(),
                    prelude,
                ));
                self.section.push((line, text));
            }
            What::Func(func) => self.func(seq, line, text, func),
            What::Decl(decl) => self.decl(seq, line, decl, mode),
            What::Stmt { form, semi } => self.statement(line, text, form, *semi, last, callable),
        }
    }

    fn func(&mut self, seq: u64, line: u32, text: &str, func: &Func) {
        self.section.push((line, text.to_string()));
        if func.local {
            self.hidden.push(func.name.clone());
            return;
        }
        let mut def = def(
            seq,
            Kind::Func,
            alloc::vec![func.name.clone()],
            func.name.clone(),
            format!("{}\n", func.proto),
        );
        def.nullary = func.nullary;
        self.defs.push(def);
    }

    fn statement(
        &mut self,
        line: u32,
        text: &str,
        form: &Form,
        semi: bool,
        last: bool,
        callable: &dyn Fn(&str) -> bool,
    ) {
        let body = text.strip_suffix(';').unwrap_or(text).trim_end();
        let shown = last && !semi;
        let text = match form {
            Form::Str => format!("{PRINT}({body});"),
            Form::Lone(name) if callable(name) && shown => {
                self.fallback = Some(format!("{name}();"));
                format!("{SHOW}(({name}()));")
            }
            Form::Lone(name) if callable(name) => format!("{name}();"),
            Form::Lone(_) | Form::Expr if shown => {
                self.fallback = Some(format!("{body};"));
                format!("{SHOW}(({body}));")
            }
            _ if !semi => format!("{text};"),
            _ => text.to_string(),
        };
        self.statements.push((line, text));
    }

    fn decl(&mut self, seq: u64, line: u32, decl: &Decl, mode: Mode) {
        for tag in &decl.tags {
            let mut keys = alloc::vec![format!("tag {}", tag.name)];
            keys.extend(tag.enumerators.iter().cloned());
            let name = format!("{} {}", tag.kind, tag.name);
            let prelude = format!("{}\n", tag.text);
            self.defs.push(def(seq, Kind::Type, keys, name, prelude));
            self.section.push((line, tag.text.clone()));
        }
        let spec = &decl.specifiers;
        if decl.declarators.is_empty() {
            if decl.anonymous {
                // `enum { A, B };`: a type whose only names are its constants.
                let text = format!("{spec};");
                let name = decl.enumerators.first().cloned().unwrap_or_default();
                let keys = decl.enumerators.clone();
                if !keys.is_empty() {
                    let prelude = format!("{text}\n");
                    self.defs.push(def(seq, Kind::Type, keys, name, prelude));
                }
                self.section.push((line, text));
            }
            return;
        }
        match decl.storage {
            Storage::Static => {
                let list = joined(&decl.declarators, true);
                self.section.push((line, format!("static {spec} {list};")));
                self.hidden
                    .extend(decl.declarators.iter().map(|d| d.name.clone()));
            }
            Storage::Typedef => self.declared(seq, line, decl, Kind::Typedef, "typedef "),
            Storage::Extern => self.declared(seq, line, decl, Kind::Decl, "extern "),
            Storage::None if decl.anonymous => {
                let list = joined(&decl.declarators, true);
                self.section.push((line, format!("{spec} {list};")));
                let mut keys: Vec<String> =
                    decl.declarators.iter().map(|d| d.name.clone()).collect();
                keys.extend(decl.enumerators.iter().cloned());
                let names = joined(&decl.declarators, false);
                let prelude = format!("extern {spec} {names};\n");
                let name = decl.declarators[0].name.clone();
                self.defs.push(def(seq, Kind::Var, keys, name, prelude));
            }
            Storage::None => {
                for d in &decl.declarators {
                    self.variable(seq, line, decl, d, mode);
                }
            }
        }
    }

    /// A typedef or an `extern`: the same text in the unit and in later preludes.
    fn declared(&mut self, seq: u64, line: u32, decl: &Decl, kind: Kind, storage: &str) {
        let spec = &decl.specifiers;
        if decl.anonymous {
            let text = format!("{storage}{spec} {};", joined(&decl.declarators, false));
            let mut keys: Vec<String> = decl.declarators.iter().map(|d| d.name.clone()).collect();
            keys.extend(decl.enumerators.iter().cloned());
            let name = decl.declarators[0].name.clone();
            self.defs
                .push(def(seq, kind, keys, name, format!("{text}\n")));
            self.section.push((line, text));
            return;
        }
        for d in &decl.declarators {
            let text = format!("{storage}{spec} {};", d.text);
            let keys = alloc::vec![d.name.clone()];
            self.defs
                .push(def(seq, kind, keys, d.name.clone(), format!("{text}\n")));
            self.section.push((line, text));
        }
    }

    /// One declarator of a declaration with no storage class.
    fn variable(&mut self, seq: u64, line: u32, decl: &Decl, d: &Declarator, mode: Mode) {
        let spec = &decl.specifiers;
        let keys = alloc::vec![d.name.clone()];
        if d.function {
            let text = format!("{spec} {};", d.text);
            self.defs.push(def(
                seq,
                Kind::Decl,
                keys,
                d.name.clone(),
                format!("{text}\n"),
            ));
            self.section.push((line, text));
            return;
        }
        let prelude = format!("extern {spec} {};\n", d.text);
        self.defs
            .push(def(seq, Kind::Var, keys, d.name.clone(), prelude));
        let assignable = !decl.constant && !d.array;
        match d.init.as_deref() {
            Some(init) if assignable && !init.starts_with('{') => {
                self.section.push((line, format!("{spec} {};", d.text)));
                if mode == Mode::Eval {
                    self.statements
                        .push((line, format!("{} = {init};", d.name)));
                }
            }
            Some(init) => self
                .section
                .push((line, format!("{spec} {} = {init};", d.text))),
            None => self.section.push((line, format!("{spec} {};", d.text))),
        }
    }
}

/// Declarators joined by `, `, with their initializers or without.
fn joined(declarators: &[Declarator], inits: bool) -> String {
    let parts: Vec<String> = declarators
        .iter()
        .map(|d| match (&d.init, inits) {
            (Some(init), true) => format!("{} = {init}", d.text),
            _ => d.text.clone(),
        })
        .collect();
    parts.join(", ")
}
