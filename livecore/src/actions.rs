//! `kai-actions 1`, an app's action rows (ADR 0003 section 5), as the build service
//! writes them into the `kai.actions` section and the OS reads them. The rows feed
//! ADR 0002's registry with owner `App(id)`; `appabi` types each export from its
//! `sig`.
//!
//! ```text
//! kai-actions 1
//! world app:notes/notes@1.0.0
//! action notes_add
//! export app:notes/actions@1.0.0#add
//! doc Add a note and return its id.
//! sig func(text: string) -> result<u32, string>
//! arg text text max=2000 role=payload from-request
//! facts read-only=no destructive=no idempotent=no open-world=no
//! inverse snapshot
//! end
//! ```
//!
//! After `world`, an optional `effects <row> ...` line names the OS rows the
//! app's effects may call (P4): any other effect is refused.
//!
//! Per action, in this order: `action`, `export`, `doc`, any `type` lines, `sig`,
//! one `arg` per parameter in order, optional `facts`, optional `cues`, `inverse`,
//! `end`. `type <name> = record|enum|variant|flags { ... }` names a type `sig` and
//! later `type` lines may use. An `arg` kind is `text`, `int`, `bool`, `path`,
//! `host`, `enum`, `list:<kind>` or `option:<kind>`; options are `max=`, `min=`,
//! `role=target|payload|option` and `from-request`. A fact left out takes ADR
//! 0002's pessimistic default.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use appabi::Ty;

/// Most actions one app may declare.
pub const MAX_ACTIONS: usize = 64;
/// Longest section, in bytes.
pub const MAX_SECTION: usize = 32 << 10;
/// Longest registry name (ADR 0002's).
pub const MAX_NAME: usize = 24;
/// Longest description.
pub const MAX_DOC: usize = 60;
const MAX_LINE: usize = 512;
const MAX_TYPES: usize = 32;
const MAX_DEPTH: usize = 8;

/// What an argument is, as the decoder can steer it (ADR 0002's argument types).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgKind {
    Text,
    Int,
    Bool,
    Path,
    Host,
    Enum,
    List(Box<ArgKind>),
    Option(Box<ArgKind>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Target,
    Payload,
    Option,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgRow {
    pub name: String,
    pub kind: ArgKind,
    pub max: Option<i64>,
    pub min: Option<i64>,
    pub role: Role,
    /// The value must come from the user's request.
    pub from_request: bool,
}

/// ADR 0002's facts; `None` is "not stated", which takes the pessimistic default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
    pub open_world: Option<bool>,
}

/// How an action is undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// The app's own checkpoint from before the call (ADR 0003 section 4).
    Snapshot,
    /// Another action of the app.
    Action(String),
    None,
}

/// One action row.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionRow {
    /// `<app>_<function>`, the registry name.
    pub name: String,
    /// The core export, such as `app:notes/actions@1.0.0#add`.
    pub export: String,
    pub doc: String,
    /// The signature, typed for `appabi`.
    pub func: appabi::Func,
    /// The parameter names, in order.
    pub params: Vec<String>,
    pub args: Vec<ArgRow>,
    pub facts: Facts,
    pub cues: Vec<String>,
    pub inverse: Inverse,
}

impl ActionRow {
    /// The parameters as a record type, the shape of a call's `args`.
    pub fn args_record(&self) -> Ty {
        Ty::Record(
            self.params
                .iter()
                .cloned()
                .zip(self.func.params.iter().cloned())
                .collect(),
        )
    }
}

/// A section: the world, the OS rows its effects may call, and its rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Actions {
    pub world: String,
    pub effects: Vec<String>,
    pub rows: Vec<ActionRow>,
}

/// Most OS rows one app's effects may name.
pub const MAX_EFFECT_ROWS: usize = 32;

impl Actions {
    pub fn by_export(&self, export: &str) -> Option<&ActionRow> {
        self.rows.iter().find(|row| row.export == export)
    }

    pub fn by_name(&self, name: &str) -> Option<&ActionRow> {
        self.rows.iter().find(|row| row.name == name)
    }
}

/// Reads a `kai-actions 1` section.
pub fn parse(text: &str) -> Result<Actions, String> {
    if text.len() > MAX_SECTION {
        return Err("the actions section is too long".into());
    }
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).peekable();
    if lines.next() != Some("kai-actions 1") {
        return Err("not a kai-actions 1 section".into());
    }
    let world = field(lines.next(), "world")?.to_string();
    let mut effects = Vec::new();
    if let Some(line) = lines.next_if(|l| l.starts_with("effects ")) {
        for name in line["effects ".len()..].split_whitespace() {
            valid_name(name)?;
            if effects.len() == MAX_EFFECT_ROWS {
                return Err(format!("more than {MAX_EFFECT_ROWS} effect rows"));
            }
            effects.push(name.to_string());
        }
    }
    let mut rows: Vec<ActionRow> = Vec::new();
    while let Some(line) = lines.next() {
        if line.len() > MAX_LINE {
            return Err("a line is too long".into());
        }
        let name = field(Some(line), "action")?;
        valid_name(name)?;
        if rows.iter().any(|r| r.name == name) {
            return Err(format!("action {name} is declared twice"));
        }
        if rows.len() == MAX_ACTIONS {
            return Err(format!("more than {MAX_ACTIONS} actions"));
        }
        rows.push(action(name, &mut lines)?);
    }
    Ok(Actions {
        world,
        effects,
        rows,
    })
}

fn field<'a>(line: Option<&'a str>, key: &str) -> Result<&'a str, String> {
    let line = line.ok_or_else(|| format!("expected {key}, got the end"))?;
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(' '))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("expected {key}, got {line:?}"))
}

fn valid_name(name: &str) -> Result<(), String> {
    let ok = name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("{name:?} is not a registry name"))
    }
}

fn action<'a>(
    name: &str,
    lines: &mut core::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Result<ActionRow, String> {
    let export = field(lines.next(), "export")?.to_string();
    let doc = field(lines.next(), "doc")?.to_string();
    if doc.len() > MAX_DOC {
        return Err(format!("{name}: the doc is over {MAX_DOC} bytes"));
    }
    let mut types: Vec<(String, Ty)> = Vec::new();
    while let Some(line) = lines.peek().copied().filter(|l| l.starts_with("type ")) {
        lines.next();
        if types.len() == MAX_TYPES {
            return Err(format!("{name}: too many types"));
        }
        types.push(type_line(line, &types)?);
    }
    let (params, func) = sig(field(lines.next(), "sig")?, &export, &types)?;
    let mut args = Vec::with_capacity(params.len());
    for (param, ty) in params.iter().zip(&func.params) {
        let arg = arg_line(field(lines.next(), "arg")?)?;
        if arg.name != *param {
            return Err(format!(
                "{name}: arg {} where the sig has {param}",
                arg.name
            ));
        }
        check_kind(&arg.kind, ty).map_err(|why| format!("{name}: arg {param}: {why}"))?;
        args.push(arg);
    }
    let mut facts = Facts::default();
    if let Some(line) = lines.peek().copied().filter(|l| l.starts_with("facts ")) {
        lines.next();
        facts = facts_line(line)?;
    }
    let mut cues = Vec::new();
    if let Some(line) = lines.peek().copied().filter(|l| l.starts_with("cues ")) {
        lines.next();
        cues = line[5..]
            .split(' ')
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect();
    }
    let inverse = match field(lines.next(), "inverse")? {
        "snapshot" => Inverse::Snapshot,
        "none" => Inverse::None,
        other => {
            valid_name(other)?;
            Inverse::Action(other.to_string())
        }
    };
    if lines.next() != Some("end") {
        return Err(format!("{name}: expected end"));
    }
    Ok(ActionRow {
        name: name.to_string(),
        export,
        doc,
        func,
        params,
        args,
        facts,
        cues,
        inverse,
    })
}

fn arg_line(line: &str) -> Result<ArgRow, String> {
    let mut words = line.split(' ').filter(|w| !w.is_empty());
    let name = words.next().ok_or("an arg with no name")?.to_string();
    let kind = arg_kind(words.next().ok_or("an arg with no kind")?, 0)?;
    let mut arg = ArgRow {
        name,
        kind,
        max: None,
        min: None,
        role: Role::Payload,
        from_request: false,
    };
    for word in words {
        match word.split_once('=') {
            Some(("max", v)) => arg.max = Some(v.parse().map_err(|_| format!("bad max {v}"))?),
            Some(("min", v)) => arg.min = Some(v.parse().map_err(|_| format!("bad min {v}"))?),
            Some(("role", "target")) => arg.role = Role::Target,
            Some(("role", "payload")) => arg.role = Role::Payload,
            Some(("role", "option")) => arg.role = Role::Option,
            None if word == "from-request" => arg.from_request = true,
            _ => return Err(format!("unknown arg option {word:?}")),
        }
    }
    Ok(arg)
}

fn arg_kind(word: &str, depth: usize) -> Result<ArgKind, String> {
    if depth > MAX_DEPTH {
        return Err("an arg kind nests too deeply".into());
    }
    Ok(match word {
        "text" => ArgKind::Text,
        "int" => ArgKind::Int,
        "bool" => ArgKind::Bool,
        "path" => ArgKind::Path,
        "host" => ArgKind::Host,
        "enum" => ArgKind::Enum,
        w => match w.split_once(':') {
            Some(("list", inner)) => ArgKind::List(Box::new(arg_kind(inner, depth + 1)?)),
            Some(("option", inner)) => ArgKind::Option(Box::new(arg_kind(inner, depth + 1)?)),
            _ => return Err(format!("unknown arg kind {w:?}")),
        },
    })
}

/// An argument kind fits its WIT type (ADR 0003 section 5's subset).
fn check_kind(kind: &ArgKind, ty: &Ty) -> Result<(), String> {
    let ok = match (kind, ty) {
        (ArgKind::Text | ArgKind::Path | ArgKind::Host, Ty::String) => true,
        (ArgKind::Bool, Ty::Bool) | (ArgKind::Enum, Ty::Enum(_)) => true,
        (
            ArgKind::Int,
            Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::S8 | Ty::S16 | Ty::S32 | Ty::S64,
        ) => true,
        (ArgKind::List(k), Ty::List(t)) | (ArgKind::Option(k), Ty::Option(t)) => {
            return check_kind(k, t);
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("kind {kind:?} does not fit {ty:?}"))
    }
}

fn facts_line(line: &str) -> Result<Facts, String> {
    let mut facts = Facts::default();
    for word in line[6..].split(' ').filter(|w| !w.is_empty()) {
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| format!("bad fact {word:?}"))?;
        let value = match value {
            "yes" => true,
            "no" => false,
            _ => return Err(format!("bad fact {word:?}")),
        };
        let slot = match key {
            "read-only" => &mut facts.read_only,
            "destructive" => &mut facts.destructive,
            "idempotent" => &mut facts.idempotent,
            "open-world" => &mut facts.open_world,
            _ => return Err(format!("unknown fact {key:?}")),
        };
        *slot = Some(value);
    }
    Ok(facts)
}

/// `sig func(a: T, ...) -> R`: the parameter names and the typed function.
fn sig(
    text: &str,
    export: &str,
    types: &[(String, Ty)],
) -> Result<(Vec<String>, appabi::Func), String> {
    let mut p = Wit { text, at: 0, types };
    p.expect("func")?;
    p.expect("(")?;
    let mut names = Vec::new();
    let mut params = Vec::new();
    if !p.eat(")") {
        loop {
            let name = p.ident()?;
            if names.contains(&name) {
                return Err(format!("parameter {name} twice"));
            }
            p.expect(":")?;
            params.push(p.ty(0)?);
            names.push(name);
            if p.eat(")") {
                break;
            }
            p.expect(",")?;
        }
    }
    let result = if p.eat("->") { Some(p.ty(0)?) } else { None };
    p.end()?;
    Ok((
        names,
        appabi::Func {
            export: export.to_string(),
            params,
            result,
        },
    ))
}

/// `type <name> = record|enum|variant|flags { ... }`.
fn type_line(line: &str, types: &[(String, Ty)]) -> Result<(String, Ty), String> {
    let mut p = Wit {
        text: line,
        at: 0,
        types,
    };
    p.expect("type")?;
    let name = p.ident()?;
    if types.iter().any(|(n, _)| *n == name) {
        return Err(format!("type {name} twice"));
    }
    p.expect("=")?;
    let kind = p.ident()?;
    p.expect("{")?;
    let mut entries: Vec<(String, Option<Ty>)> = Vec::new();
    if !p.eat("}") {
        loop {
            let entry = p.ident()?;
            let payload = match kind.as_str() {
                "record" => {
                    p.expect(":")?;
                    Some(p.ty(0)?)
                }
                "variant" if p.eat("(") => {
                    let t = p.ty(0)?;
                    p.expect(")")?;
                    Some(t)
                }
                _ => None,
            };
            if entries.iter().any(|(n, _)| *n == entry) {
                return Err(format!("{entry} twice in {name}"));
            }
            entries.push((entry, payload));
            if p.eat("}") {
                break;
            }
            p.expect(",")?;
            if p.eat("}") {
                break;
            }
        }
    }
    p.end()?;
    let names = || entries.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    let ty = match kind.as_str() {
        "record" => Ty::Record(
            entries
                .iter()
                .map(|(n, t)| (n.clone(), t.clone().unwrap()))
                .collect(),
        ),
        "variant" => Ty::Variant(entries.clone()),
        "enum" => Ty::Enum(names()),
        "flags" => Ty::Flags(names()),
        other => return Err(format!("unknown type kind {other}")),
    };
    Ok((name, ty))
}

/// A cursor over the WIT subset `sig` and `type` lines use.
struct Wit<'a> {
    text: &'a str,
    at: usize,
    types: &'a [(String, Ty)],
}

impl Wit<'_> {
    fn space(&mut self) {
        let rest = &self.text[self.at..];
        self.at += rest.len() - rest.trim_start().len();
    }

    fn eat(&mut self, token: &str) -> bool {
        self.space();
        if self.text[self.at..].starts_with(token) {
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
            Err(format!("expected {token:?} in {:?}", self.text))
        }
    }

    fn end(&mut self) -> Result<(), String> {
        self.space();
        if self.at == self.text.len() {
            Ok(())
        } else {
            Err(format!("unexpected text in {:?}", self.text))
        }
    }

    /// A kebab-case WIT identifier.
    fn ident(&mut self) -> Result<String, String> {
        self.space();
        let rest = &self.text[self.at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        if len == 0 || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Err(format!("expected a name in {:?}", self.text));
        }
        self.at += len;
        Ok(rest[..len].to_string())
    }

    fn ty(&mut self, depth: usize) -> Result<Ty, String> {
        if depth > MAX_DEPTH {
            return Err("a type nests too deeply".into());
        }
        let name = self.ident()?;
        let inner = |p: &mut Self| -> Result<Box<Ty>, String> { Ok(Box::new(p.ty(depth + 1)?)) };
        Ok(match name.as_str() {
            "bool" => Ty::Bool,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "s8" => Ty::S8,
            "s16" => Ty::S16,
            "s32" => Ty::S32,
            "s64" => Ty::S64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "char" => Ty::Char,
            "string" => Ty::String,
            "list" | "option" => {
                self.expect("<")?;
                let t = inner(self)?;
                self.expect(">")?;
                if name == "list" {
                    Ty::List(t)
                } else {
                    Ty::Option(t)
                }
            }
            "tuple" => {
                self.expect("<")?;
                let mut items = alloc::vec![self.ty(depth + 1)?];
                while self.eat(",") {
                    items.push(self.ty(depth + 1)?);
                }
                self.expect(">")?;
                Ty::Tuple(items)
            }
            "result" => {
                if !self.eat("<") {
                    return Ok(Ty::Result(None, None));
                }
                let ok = if self.eat("_") {
                    None
                } else {
                    Some(inner(self)?)
                };
                let err = if self.eat(",") {
                    Some(inner(self)?)
                } else {
                    None
                };
                self.expect(">")?;
                Ty::Result(ok, err)
            }
            other => self
                .types
                .iter()
                .find(|(n, _)| n == other)
                .map(|(_, t)| t.clone())
                .ok_or_else(|| format!("unknown type {other}"))?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const NOTES: &str = "kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
inverse snapshot
end
action notes_find
export app:notes/actions@1.0.0#find
doc Find notes with a word.
type hit = record { id: u32, words: u32 }
type order = enum { newest, oldest }
sig func(word: string, order: order) -> list<hit>
arg word text max=60 role=target from-request
arg order enum role=option
facts read-only=yes destructive=no idempotent=yes open-world=no
cues find search look
inverse none
end
";

    #[test]
    fn notes_rows_parse() {
        let actions = parse(NOTES).unwrap();
        assert_eq!(actions.world, "app:notes/notes@1.0.0");
        assert_eq!(actions.rows.len(), 2);
        let add = actions.by_name("notes_add").unwrap();
        assert_eq!(add.func.export, "app:notes/actions@1.0.0#add");
        assert_eq!(add.func.params, alloc::vec![Ty::String]);
        assert_eq!(
            add.func.result,
            Some(Ty::Result(
                Some(Box::new(Ty::U32)),
                Some(Box::new(Ty::String))
            ))
        );
        assert_eq!(add.args[0].max, Some(2000));
        assert!(add.args[0].from_request);
        assert_eq!(add.inverse, Inverse::Snapshot);
        assert_eq!(add.facts.read_only, Some(false));
        let find = actions.by_export("app:notes/actions@1.0.0#find").unwrap();
        assert_eq!(find.cues, ["find", "search", "look"]);
        assert_eq!(find.args[1].role, Role::Option);
        let Some(Ty::List(hit)) = &find.func.result else {
            panic!()
        };
        assert!(matches!(**hit, Ty::Record(ref f) if f.len() == 2));
        assert_eq!(
            find.args_record(),
            Ty::Record(alloc::vec![
                (String::from("word"), Ty::String),
                (
                    String::from("order"),
                    Ty::Enum(alloc::vec![String::from("newest"), String::from("oldest")])
                ),
            ])
        );
    }

    #[test]
    fn an_effects_line_names_the_rows_effects_may_call() {
        assert!(parse(NOTES).unwrap().effects.is_empty());
        let text = NOTES.replacen(
            "world app:notes/notes@1.0.0
",
            "world app:notes/notes@1.0.0
effects fs_list fs_read
",
            1,
        );
        assert_eq!(parse(&text).unwrap().effects, ["fs_list", "fs_read"]);
        let bad = text.replace("fs_read", "Fs-Read");
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn a_fact_left_out_is_unknown() {
        let text = NOTES.replace(
            "facts read-only=no destructive=no idempotent=no open-world=no\n",
            "facts read-only=no\n",
        );
        let add = parse(&text).unwrap().rows.remove(0);
        assert_eq!(add.facts.read_only, Some(false));
        assert_eq!(add.facts.destructive, None);
    }

    #[test]
    fn bad_rows_are_refused() {
        for (from, to) in [
            ("kai-actions 1", "kai-actions 2"),
            ("action notes_add", "action Notes-Add"),
            ("action notes_add", "action notes_add_a_name_far_too_long"),
            ("arg text text", "arg body text"),
            ("arg text text", "arg text int"),
            ("arg order enum", "arg order text"),
            ("sig func(text: string)", "sig func(text: strings)"),
            (
                "sig func(text: string)",
                "sig func(text: string, text: u32)",
            ),
            ("inverse snapshot", "inverse Undo!"),
            (
                "open-world=no\ninverse snapshot",
                "open-world=maybe\ninverse snapshot",
            ),
            ("max=2000", "max=lots"),
            (
                "Add a note and return its id.",
                "Add a note and return its id, with a far longer sentence than any doc may hold.",
            ),
            ("end\naction notes_find", "action notes_find"),
            ("type order = enum", "type order = union"),
        ] {
            let text = NOTES.replacen(from, to, 1);
            assert!(parse(&text).is_err(), "{to}");
        }
        let twice = NOTES.replace("action notes_find", "action notes_add");
        assert!(parse(&twice).is_err());
    }
}
