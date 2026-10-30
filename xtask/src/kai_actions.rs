//! The `kai.actions` writer (ADR 0003 section 5): an app's action rows as
//! `kai-actions 1`, from its WIT. Every function of every interface the app's
//! world exports from its own package is an action. The function's first doc line
//! is its description; `kai:` doc lines give its facts, inverse and cues, and
//! `kai: arg <name> <options>` an argument's bounds and role:
//!
//! ```text
//! /// Add a note and return its id.
//! /// kai: read-only=no destructive=no idempotent=no open-world=no inverse=snapshot cues=add,note
//! /// kai: arg text max=2000 role=payload from-request
//! add: func(text: string) -> result<u32, string>;
//! ```
//!
//! The OS reads the section with `livecore::actions::parse`, and so does the
//! writer before it returns, so a row the OS would refuse never ships.

use crate::wit;
use std::fmt::Write as _;
use std::path::PathBuf;
use wit_parser::{Function, Resolve, Type, TypeDefKind, TypeId, WorldItem};

/// The `kai-actions 1` section for `world` (such as `app:notes/notes@1.0.0`),
/// resolving the WIT packages in `dirs`.
pub fn write(dirs: &[PathBuf], world: &str) -> Result<String, String> {
    let (resolve, packages) = wit::load_dirs(dirs)?;
    let (package, world_name) = world
        .split_once('/')
        .and_then(|(ns_name, rest)| {
            let (name, version) = rest.split_once('@')?;
            Some((format!("{ns_name}@{version}"), name))
        })
        .ok_or_else(|| format!("{world} is not <namespace>:<package>/<world>@<version>"))?;
    let package_id = *packages
        .get(&package)
        .ok_or_else(|| format!("no WIT package {package}"))?;
    let world_id = *resolve.packages[package_id]
        .worlds
        .get(world_name)
        .ok_or_else(|| format!("{package} has no world {world_name}"))?;
    let mut out = format!("kai-actions 1\nworld {world}\n");
    // The OS rows the app's effects may call, from a `kai: effects=a,b` line
    // of the world's doc comment (P4): none if it has none.
    let effects = world_effects(resolve.worlds[world_id].docs.contents.as_deref());
    if !effects.is_empty() {
        out.push_str(&format!("effects {}\n", effects.join(" ")));
    }
    for item in resolve.worlds[world_id].exports.values() {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let iface = &resolve.interfaces[*id];
        // The app's own actions, which may come from another app package (as
        // `notes-window` exports `app:notes/actions`); never the system's.
        let Some(owner) = iface.package else {
            continue;
        };
        let app = &resolve.packages[owner].name;
        if app.namespace == "kai" {
            continue;
        }
        let iface_name = iface
            .name
            .as_deref()
            .ok_or("an unnamed exported interface")?;
        for func in iface.functions.values() {
            let export = format!(
                "{}:{}/{iface_name}@{}#{}",
                app.namespace,
                app.name,
                version(app)?,
                func.name
            );
            let name = format!("{}_{}", app.name, func.name).replace('-', "_");
            out.push_str(&Row::new(&resolve).action(&name, &export, func)?);
        }
    }
    livecore::actions::parse(&out).map_err(|why| format!("{world}: {why}"))?;
    Ok(out)
}

/// The rows a world's `kai: effects=a,b` doc line names.
fn world_effects(docs: Option<&str>) -> Vec<String> {
    docs.unwrap_or("")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("kai: effects="))
        .flat_map(|list| list.split(','))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

fn version(name: &wit_parser::PackageName) -> Result<String, String> {
    name.version
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| format!("{name} has no version"))
}

/// One action's lines, with the named types its signature uses.
struct Row<'a> {
    resolve: &'a Resolve,
    /// `type` lines, each after the ones it uses.
    types: Vec<(TypeId, String)>,
}

impl<'a> Row<'a> {
    fn new(resolve: &'a Resolve) -> Self {
        Self {
            resolve,
            types: Vec::new(),
        }
    }

    fn action(mut self, name: &str, export: &str, func: &Function) -> Result<String, String> {
        let docs = func.docs.contents.as_deref().unwrap_or("");
        let mut lines = docs.lines().map(str::trim);
        let doc = lines
            .next()
            .filter(|line| !line.is_empty() && !line.starts_with("kai:"))
            .ok_or_else(|| format!("{export}: the first doc line must describe it"))?;
        let mut facts = Vec::new();
        let (mut inverse, mut cues) = (None, Vec::new());
        let mut arg_options: Vec<(&str, &str)> = Vec::new();
        for line in lines.filter_map(|line| line.strip_prefix("kai:")) {
            let line = line.trim();
            if let Some(arg) = line.strip_prefix("arg ") {
                let (arg, options) = arg.split_once(' ').unwrap_or((arg, ""));
                arg_options.push((arg, options.trim()));
                continue;
            }
            for word in line.split_whitespace() {
                match word.split_once('=') {
                    Some(("inverse", value)) => inverse = Some(value),
                    Some(("cues", value)) => cues.extend(value.split(',')),
                    Some(_) => facts.push(word),
                    None => return Err(format!("{export}: unknown kai: word {word:?}")),
                }
            }
        }
        let mut params = Vec::new();
        let mut args = String::new();
        for param in &func.params {
            params.push(format!("{}: {}", param.name, self.ty(&param.ty)?));
            let kind = self.arg_kind(&param.ty)?;
            let options = arg_options
                .iter()
                .find(|(arg, _)| *arg == param.name)
                .map_or("", |(_, options)| options);
            let _ = writeln!(args, "arg {} {kind} {options}", param.name);
        }
        if let Some((arg, _)) = arg_options
            .iter()
            .find(|(arg, _)| !func.params.iter().any(|p| p.name == *arg))
        {
            return Err(format!("{export}: kai: arg {arg} names no parameter"));
        }
        let result = match &func.result {
            Some(t) => format!(" -> {}", self.ty(t)?),
            None => String::new(),
        };
        let mut out = format!("action {name}\nexport {export}\ndoc {doc}\n");
        for (_, line) in &self.types {
            let _ = writeln!(out, "{line}");
        }
        let _ = writeln!(out, "sig func({}){result}", params.join(", "));
        out.push_str(&args);
        if !facts.is_empty() {
            let _ = writeln!(out, "facts {}", facts.join(" "));
        }
        if !cues.is_empty() {
            let _ = writeln!(out, "cues {}", cues.join(" "));
        }
        let _ = writeln!(out, "inverse {}\nend", inverse.unwrap_or("none"));
        Ok(out)
    }

    /// A type as `sig` writes it; a named record, enum, variant or flags adds its
    /// `type` line.
    fn ty(&mut self, t: &Type) -> Result<String, String> {
        Ok(match t {
            Type::Bool => "bool".into(),
            Type::U8 => "u8".into(),
            Type::U16 => "u16".into(),
            Type::U32 => "u32".into(),
            Type::U64 => "u64".into(),
            Type::S8 => "s8".into(),
            Type::S16 => "s16".into(),
            Type::S32 => "s32".into(),
            Type::S64 => "s64".into(),
            Type::F32 => "f32".into(),
            Type::F64 => "f64".into(),
            Type::Char => "char".into(),
            Type::String => "string".into(),
            Type::Id(id) => self.id(*id)?,
            other => return Err(format!("{other:?} is outside the actions subset")),
        })
    }

    fn id(&mut self, id: TypeId) -> Result<String, String> {
        let def = &self.resolve.types[id];
        let named = |kind: &str| -> Result<String, String> {
            def.name
                .clone()
                .ok_or_else(|| format!("an anonymous {kind}"))
        };
        Ok(match &def.kind {
            TypeDefKind::Type(t) => self.ty(t)?,
            TypeDefKind::List(t) => format!("list<{}>", self.ty(t)?),
            TypeDefKind::Option(t) => format!("option<{}>", self.ty(t)?),
            TypeDefKind::Tuple(t) => {
                let items = t
                    .types
                    .iter()
                    .map(|t| self.ty(t))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("tuple<{}>", items.join(", "))
            }
            TypeDefKind::Result(r) => match (&r.ok, &r.err) {
                (None, None) => "result".into(),
                (Some(ok), None) => format!("result<{}>", self.ty(ok)?),
                (None, Some(err)) => format!("result<_, {}>", self.ty(err)?),
                (Some(ok), Some(err)) => format!("result<{}, {}>", self.ty(ok)?, self.ty(err)?),
            },
            TypeDefKind::Record(r) => {
                let name = named("record")?;
                let fields = r
                    .fields
                    .iter()
                    .map(|f| Ok(format!("{}: {}", f.name, self.ty(&f.ty)?)))
                    .collect::<Result<Vec<_>, String>>()?;
                self.define(
                    id,
                    format!("type {name} = record {{ {} }}", fields.join(", ")),
                );
                name
            }
            TypeDefKind::Variant(v) => {
                let name = named("variant")?;
                let cases = v
                    .cases
                    .iter()
                    .map(|c| match &c.ty {
                        Some(t) => Ok(format!("{}({})", c.name, self.ty(t)?)),
                        None => Ok(c.name.clone()),
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                self.define(
                    id,
                    format!("type {name} = variant {{ {} }}", cases.join(", ")),
                );
                name
            }
            TypeDefKind::Enum(e) => {
                let name = named("enum")?;
                let cases: Vec<&str> = e.cases.iter().map(|c| c.name.as_str()).collect();
                self.define(id, format!("type {name} = enum {{ {} }}", cases.join(", ")));
                name
            }
            TypeDefKind::Flags(f) => {
                let name = named("flags")?;
                let flags: Vec<&str> = f.flags.iter().map(|c| c.name.as_str()).collect();
                self.define(
                    id,
                    format!("type {name} = flags {{ {} }}", flags.join(", ")),
                );
                name
            }
            other => return Err(format!("{} is outside the actions subset", other.as_str())),
        })
    }

    fn define(&mut self, id: TypeId, line: String) {
        if !self.types.iter().any(|(known, _)| *known == id) {
            self.types.push((id, line));
        }
    }

    /// The decoder's kind for a parameter's type.
    fn arg_kind(&self, t: &Type) -> Result<String, String> {
        Ok(match t {
            Type::String => "text".into(),
            Type::Bool => "bool".into(),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 => "int".into(),
            Type::S8 | Type::S16 | Type::S32 | Type::S64 => "int".into(),
            Type::Id(id) => match &self.resolve.types[*id].kind {
                TypeDefKind::Type(t) => self.arg_kind(t)?,
                TypeDefKind::List(t) => format!("list:{}", self.arg_kind(t)?),
                TypeDefKind::Option(t) => format!("option:{}", self.arg_kind(t)?),
                TypeDefKind::Enum(_) => "enum".into(),
                other => return Err(format!("a {} argument has no decoder kind", other.as_str())),
            },
            other => return Err(format!("a {other:?} argument has no decoder kind")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use livecore::actions::{Inverse, Role};

    fn notes_dirs() -> Vec<PathBuf> {
        let root = crate::workspace_root();
        vec![
            root.join("wit/kai-live"),
            root.join("wit/kai-app"),
            root.join("components/notes/wit"),
        ]
    }

    #[test]
    fn the_notes_rows_come_from_its_wit() {
        let text = write(&notes_dirs(), "app:notes/notes@1.0.0").unwrap();
        let actions = livecore::actions::parse(&text).unwrap();
        assert_eq!(actions.world, "app:notes/notes@1.0.0");
        // Notes keeps its notes in its own storage: its effects call no row.
        assert!(actions.effects.is_empty());
        let names: Vec<&str> = actions.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "notes_add",
                "notes_count",
                "notes_find",
                "notes_get",
                "notes_about"
            ]
        );
        let add = actions.by_name("notes_add").unwrap();
        assert_eq!(add.export, "app:notes/actions@1.0.0#add");
        assert_eq!(add.inverse, Inverse::Snapshot);
        assert_eq!(add.facts.read_only, Some(false));
        assert_eq!(add.args[0].max, Some(2000));
        assert!(add.args[0].from_request);
        assert_eq!(add.cues, ["add", "note", "remember", "write"]);
        let get = actions.by_name("notes_get").unwrap();
        assert_eq!((get.args[0].min, get.args[0].role), (Some(1), Role::Target));
        assert!(
            text.contains("type tally = record { notes: u32, words: u32 }\nsig func() -> tally\n")
        );
        assert!(text.contains("sig func(word: string) -> list<u32>\n"));
        assert!(text.contains("sig func(id: u32) -> option<string>\n"));
    }

    #[test]
    fn a_worlds_effects_line_comes_from_its_docs() {
        let docs = "The Files app.\nkai: effects=fs_list, fs_read,fs_move\n";
        assert_eq!(world_effects(Some(docs)), ["fs_list", "fs_read", "fs_move"]);
        assert!(world_effects(None).is_empty());
    }

    #[test]
    fn a_world_that_is_not_there_is_refused() {
        assert!(write(&notes_dirs(), "app:notes/other@1.0.0").is_err());
        assert!(write(&notes_dirs(), "app:other/notes@1.0.0").is_err());
        assert!(write(&notes_dirs(), "notes").is_err());
    }
}
