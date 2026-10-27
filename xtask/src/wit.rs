//! `cargo xtask wit-check [--freeze]`: the freeze rule for the in-tree WIT packages
//! (ADR 0003, section 3).
//!
//! Every package under `wit/` has a frozen copy, `wit/frozen/<namespace>-<name>@<version>.wit`.
//! The check resolves both with wit-parser and compares them item by item: every
//! frozen interface, world, type and function must still be there with the same
//! shape, and anything new must carry `@since(version = ...)` newer than the frozen
//! version (stable AIDL's rule). Doc comments may change. `--freeze` writes the
//! frozen copy of a package that has none; it never overwrites one.
//!
//! The xtask tests run the check over the tree, so CI enforces it on every PR.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use wit_parser::{
    Function, FunctionKind, Handle, InterfaceId, PackageId, Resolve, Stability, Type, TypeDefKind,
    TypeId, UnresolvedPackageGroup, WorldItem, WorldKey,
};

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root().join("wit");
    match args {
        [] => {
            let checked = check(&root)?;
            println!("xtask: wit-check: {checked} packages match their frozen copies");
            Ok(())
        }
        [flag] if flag == "--freeze" => {
            for written in freeze(&root)? {
                println!("xtask: wit-check: froze {}", written.display());
            }
            check(&root).map(|_| ())
        }
        _ => Err("usage: cargo xtask wit-check [--freeze]".into()),
    }
}

/// Compares every frozen package with the tree; how many there were.
///
/// A frozen copy is compared with the tree's package of the same name and a
/// compatible version (the same `0.minor` while 0.x, then the same major), which
/// may be newer: additions move the version on and carry `@since` it.
pub fn check(root: &Path) -> Result<usize, String> {
    let (tree, packages) = load_tree(root)?;
    let (frozen, frozen_packages) = load_frozen(&root.join("frozen"))?;
    for (name, &id) in &packages {
        let line = Line::of(&tree, id);
        if !frozen_packages
            .values()
            .any(|&old| Line::of(&frozen, old).compatible(&line))
        {
            return Err(format!(
                "{name} has no frozen copy; run `cargo xtask wit-check --freeze`"
            ));
        }
    }
    let mut problems = Vec::new();
    for (name, &old) in &frozen_packages {
        let line = Line::of(&frozen, old);
        let Some(&new) = packages
            .values()
            .find(|&&new| line.compatible(&Line::of(&tree, new)))
        else {
            problems.push(format!(
                "{name} is frozen but wit/ has no compatible version"
            ));
            continue;
        };
        if Line::of(&tree, new).version < line.version {
            problems.push(format!("{name} is frozen but wit/ has an older version"));
            continue;
        }
        let pair = Pair {
            old: &frozen,
            new: &tree,
            version: line.version.clone(),
            problems: &mut problems,
        };
        pair.packages(old, new, name);
    }
    if problems.is_empty() {
        Ok(frozen_packages.len())
    } else {
        Err(format!(
            "wit-check: {} change(s) break the freeze:\n  {}",
            problems.len(),
            problems.join("\n  ")
        ))
    }
}

/// A package's name without its version, and its version.
struct Line {
    name: String,
    version: Option<semver::Version>,
}

impl Line {
    fn of(resolve: &Resolve, id: PackageId) -> Self {
        let name = &resolve.packages[id].name;
        Self {
            name: format!("{}:{}", name.namespace, name.name),
            version: name.version.clone(),
        }
    }

    /// Same name, and versions that link: `0.minor` while 0.x, then the major.
    fn compatible(&self, other: &Self) -> bool {
        let canonical = |v: &Option<semver::Version>| {
            v.as_ref().map(|v| match v.major {
                0 => (0, v.minor),
                major => (major, 0),
            })
        };
        self.name == other.name && canonical(&self.version) == canonical(&other.version)
    }
}

/// Writes a frozen copy of every package that has none of a compatible version.
pub fn freeze(root: &Path) -> Result<Vec<PathBuf>, String> {
    let (tree, packages) = load_tree(root)?;
    let frozen = root.join("frozen");
    fs::create_dir_all(&frozen).map_err(|e| format!("{}: {e}", frozen.display()))?;
    let (old, old_packages) = load_frozen(&frozen)?;
    let mut written = Vec::new();
    for (name, dir) in package_dirs(root)? {
        let Some(&id) = packages.get(&name) else {
            continue;
        };
        let line = Line::of(&tree, id);
        if old_packages
            .values()
            .any(|&o| Line::of(&old, o).compatible(&line))
        {
            continue;
        }
        let path = frozen.join(frozen_file_name(&name));
        fs::write(&path, package_text(&dir)?).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn frozen_file_name(name: &str) -> String {
    format!("{}.wit", name.replacen(':', "-", 1))
}

/// The text of a package directory as one file: its `.wit` files in name order,
/// with the `package` line kept only from the first.
fn package_text(dir: &Path) -> Result<String, String> {
    let mut text = String::new();
    for (index, path) in wit_files(dir)?.iter().enumerate() {
        let file = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in file.lines() {
            if index > 0 && line.trim_start().starts_with("package ") {
                continue;
            }
            text.push_str(line);
            text.push('\n');
        }
    }
    Ok(text)
}

fn wit_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wit"))
        .collect();
    files.sort();
    Ok(files)
}

/// Every package directory under `root` (all but `frozen`), keyed by package name.
fn package_dirs(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.file_name().is_some_and(|n| n != "frozen"))
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let group = UnresolvedPackageGroup::parse_dir(&dir)
                .map_err(|e| format!("{}: {e:#}", dir.display()))?;
            Ok((group.main.name.to_string(), dir))
        })
        .collect()
}

pub(crate) type Packages = BTreeMap<String, PackageId>;

pub(crate) fn load_tree(root: &Path) -> Result<(Resolve, Packages), String> {
    let groups = package_dirs(root)?
        .into_iter()
        .map(|(_, dir)| {
            UnresolvedPackageGroup::parse_dir(&dir).map_err(|e| format!("{}: {e:#}", dir.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    resolve(groups)
}

fn load_frozen(dir: &Path) -> Result<(Resolve, Packages), String> {
    if !dir.exists() {
        return Ok((Resolve::default(), Packages::new()));
    }
    let groups = wit_files(dir)?
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            UnresolvedPackageGroup::parse(&path, &text)
                .map_err(|(map, e)| format!("{}: {}", path.display(), e.render(&map)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    resolve(groups)
}

/// Resolves `groups` together, whatever order they depend on each other in.
fn resolve(mut groups: Vec<UnresolvedPackageGroup>) -> Result<(Resolve, Packages), String> {
    let mut resolve = Resolve::default();
    if !groups.is_empty() {
        let main = groups.remove(0);
        resolve
            .push_groups(main, groups)
            .map_err(|e| format!("resolving WIT: {e:#}"))?;
    }
    let packages = resolve
        .packages
        .iter()
        .map(|(id, package)| (package.name.to_string(), id))
        .collect();
    Ok((resolve, packages))
}

/// A frozen package against the tree's, collecting what breaks the freeze.
struct Pair<'a> {
    old: &'a Resolve,
    new: &'a Resolve,
    /// The frozen package's version; an addition must be `@since` a later one.
    version: Option<semver::Version>,
    problems: &'a mut Vec<String>,
}

impl Pair<'_> {
    fn packages(mut self, old: PackageId, new: PackageId, name: &str) {
        let (old_pkg, new_pkg) = (&self.old.packages[old], &self.new.packages[new]);
        for (iface, &old_id) in &old_pkg.interfaces {
            match new_pkg.interfaces.get(iface) {
                Some(&new_id) => self.interfaces(old_id, new_id, &format!("{name} {iface}")),
                None => self.problem(format!("{name}: interface {iface} was removed")),
            }
        }
        for (iface, &new_id) in &new_pkg.interfaces {
            if !old_pkg.interfaces.contains_key(iface) {
                let stability = self.new.interfaces[new_id].stability.clone();
                self.added(&stability, &format!("{name}: interface {iface}"));
            }
        }
        for (world, &old_id) in &old_pkg.worlds {
            let Some(&new_id) = new_pkg.worlds.get(world) else {
                self.problem(format!("{name}: world {world} was removed"));
                continue;
            };
            let (old_w, new_w) = (&self.old.worlds[old_id], &self.new.worlds[new_id]);
            let at = format!("{name} world {world}");
            self.world_items(&old_w.imports, &new_w.imports, &format!("{at} import"));
            self.world_items(&old_w.exports, &new_w.exports, &format!("{at} export"));
        }
        for (world, &new_id) in &new_pkg.worlds {
            if !old_pkg.worlds.contains_key(world) {
                let stability = self.new.worlds[new_id].stability.clone();
                self.added(&stability, &format!("{name}: world {world}"));
            }
        }
    }

    fn interfaces(&mut self, old: InterfaceId, new: InterfaceId, at: &str) {
        let (old_i, new_i) = (&self.old.interfaces[old], &self.new.interfaces[new]);
        for (name, &old_ty) in &old_i.types {
            match new_i.types.get(name) {
                Some(&new_ty) => {
                    let (was, now) = (def(self.old, old_ty), def(self.new, new_ty));
                    if was != now {
                        self.problem(format!("{at}: type {name} changed from {was} to {now}"));
                    }
                }
                None => self.problem(format!("{at}: type {name} was removed")),
            }
        }
        for (name, &new_ty) in &new_i.types {
            if !old_i.types.contains_key(name) {
                let stability = self.new.types[new_ty].stability.clone();
                self.added(&stability, &format!("{at}: type {name}"));
            }
        }
        for (name, old_f) in &old_i.functions {
            match new_i.functions.get(name) {
                Some(new_f) => self.functions(old_f, new_f, &format!("{at}: {name}")),
                None => self.problem(format!("{at}: function {name} was removed")),
            }
        }
        for (name, new_f) in &new_i.functions {
            if !old_i.functions.contains_key(name) {
                self.added(&new_f.stability, &format!("{at}: function {name}"));
            }
        }
    }

    fn functions(&mut self, old: &Function, new: &Function, at: &str) {
        let (was, now) = (signature(self.old, old), signature(self.new, new));
        if was != now {
            self.problem(format!("{at} changed from {was} to {now}"));
        }
    }

    fn world_items(
        &mut self,
        old: &wit_parser::IndexMap<WorldKey, WorldItem>,
        new: &wit_parser::IndexMap<WorldKey, WorldItem>,
        at: &str,
    ) {
        let old_items: BTreeMap<String, String> = old
            .iter()
            .map(|(k, v)| (key(self.old, k), item(self.old, v)))
            .collect();
        let new_items: BTreeMap<String, (String, Stability)> = new
            .iter()
            .map(|(k, v)| {
                (
                    key(self.new, k),
                    (item(self.new, v), v.stability(self.new).clone()),
                )
            })
            .collect();
        for (name, was) in &old_items {
            match new_items.get(name) {
                Some((now, _)) if now == was => {}
                Some((now, _)) => self.problem(format!("{at} {name} changed from {was} to {now}")),
                None => self.problem(format!("{at} {name} was removed")),
            }
        }
        for (name, (_, stability)) in &new_items {
            if !old_items.contains_key(name) {
                self.added(stability, &format!("{at} {name}"));
            }
        }
    }

    /// An addition is allowed only behind `@since` a version after the frozen one.
    fn added(&mut self, stability: &Stability, what: &str) {
        match (stability, &self.version) {
            (Stability::Stable { since, .. }, Some(frozen)) if since > frozen => {}
            (Stability::Stable { since, .. }, None) if *since > semver::Version::new(0, 0, 0) => {}
            (Stability::Stable { since, .. }, _) => self.problem(format!(
                "{what} was added @since({since}), which is not after the frozen version"
            )),
            _ => self.problem(format!("{what} was added without @since(version = ...)")),
        }
    }

    fn problem(&mut self, text: String) {
        self.problems.push(text);
    }
}

fn key(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(name) => name.clone(),
        WorldKey::Interface(id) => interface_name(resolve, *id),
    }
}

fn item(resolve: &Resolve, item: &WorldItem) -> String {
    match item {
        WorldItem::Interface { id, .. } => format!("interface {}", interface_name(resolve, *id)),
        WorldItem::Function(f) => signature(resolve, f),
        WorldItem::Type { id, .. } => def(resolve, *id),
    }
}

/// `ns:pkg/iface@0.1`: by the version it links at, so a compatible bump of the
/// package does not read as a rename.
fn interface_name(resolve: &Resolve, id: InterfaceId) -> String {
    let iface = &resolve.interfaces[id];
    let (Some(name), Some(package)) = (&iface.name, iface.package) else {
        return String::from("<unnamed interface>");
    };
    let package = &resolve.packages[package].name;
    let version = match &package.version {
        Some(v) if v.major == 0 => format!("@0.{}", v.minor),
        Some(v) => format!("@{}", v.major),
        None => String::new(),
    };
    format!("{}:{}/{name}{version}", package.namespace, package.name)
}

/// A function's kind, parameters and result, with every type spelled out.
fn signature(resolve: &Resolve, f: &Function) -> String {
    let mut text = format!("{} (", function_kind(resolve, &f.kind));
    for (i, param) in f.params.iter().enumerate() {
        let comma = if i == 0 { "" } else { ", " };
        let _ = write!(text, "{comma}{}: {}", param.name, ty(resolve, &param.ty));
    }
    text.push(')');
    if let Some(result) = &f.result {
        let _ = write!(text, " -> {}", ty(resolve, result));
    }
    text
}

/// What kind of function it is, naming the resource a method belongs to.
fn function_kind(resolve: &Resolve, kind: &FunctionKind) -> String {
    let of = |id: &TypeId| ty(resolve, &Type::Id(*id));
    match kind {
        FunctionKind::Method(id)
        | FunctionKind::AsyncMethod(id)
        | FunctionKind::MethodGetter(id)
        | FunctionKind::MethodSetter(id)
        | FunctionKind::Static(id)
        | FunctionKind::AsyncStatic(id)
        | FunctionKind::StaticGetter(id)
        | FunctionKind::StaticSetter(id)
        | FunctionKind::Constructor(id) => {
            let name = format!("{kind:?}");
            let name = name.split('(').next().unwrap_or("method");
            format!("{name} of {}", of(id))
        }
        other => format!("{other:?}"),
    }
}

/// A type as a use site sees it: named types by owner and name, others spelled out.
fn ty(resolve: &Resolve, t: &Type) -> String {
    match t {
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
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => {
            let def = &resolve.types[*id];
            match (&def.name, def.owner) {
                (Some(name), wit_parser::TypeOwner::Interface(owner)) => {
                    format!("{}.{name}", interface_name(resolve, owner))
                }
                (Some(name), _) => name.clone(),
                (None, _) => kind(resolve, &def.kind),
            }
        }
    }
}

/// A named type's definition, spelled out.
fn def(resolve: &Resolve, id: TypeId) -> String {
    kind(resolve, &resolve.types[id].kind)
}

fn kind(resolve: &Resolve, kind: &TypeDefKind) -> String {
    let list = |types: &mut dyn Iterator<Item = String>| types.collect::<Vec<_>>().join(", ");
    let opt = |t: &Option<Type>| t.as_ref().map_or_else(|| "_".into(), |t| ty(resolve, t));
    match kind {
        TypeDefKind::Record(r) => format!(
            "record {{ {} }}",
            list(
                &mut r
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, ty(resolve, &f.ty)))
            )
        ),
        TypeDefKind::Resource => "resource".into(),
        TypeDefKind::Handle(Handle::Own(id)) => format!("own<{}>", ty(resolve, &Type::Id(*id))),
        TypeDefKind::Handle(Handle::Borrow(id)) => {
            format!("borrow<{}>", ty(resolve, &Type::Id(*id)))
        }
        TypeDefKind::Flags(f) => format!(
            "flags {{ {} }}",
            list(&mut f.flags.iter().map(|f| f.name.clone()))
        ),
        TypeDefKind::Tuple(t) => format!(
            "tuple<{}>",
            list(&mut t.types.iter().map(|t| ty(resolve, t)))
        ),
        TypeDefKind::Variant(v) => format!(
            "variant {{ {} }}",
            list(&mut v.cases.iter().map(|c| match &c.ty {
                Some(t) => format!("{}({})", c.name, ty(resolve, t)),
                None => c.name.clone(),
            }))
        ),
        TypeDefKind::Enum(e) => format!(
            "enum {{ {} }}",
            list(&mut e.cases.iter().map(|c| c.name.clone()))
        ),
        TypeDefKind::Option(t) => format!("option<{}>", ty(resolve, t)),
        TypeDefKind::Result(r) => format!("result<{}, {}>", opt(&r.ok), opt(&r.err)),
        TypeDefKind::List(t) => format!("list<{}>", ty(resolve, t)),
        TypeDefKind::Map(k, v) => format!("map<{}, {}>", ty(resolve, k), ty(resolve, v)),
        TypeDefKind::FixedLengthList(t, n) => format!("list<{}, {n}>", ty(resolve, t)),
        TypeDefKind::Future(t) => format!("future<{}>", opt(t)),
        TypeDefKind::Stream(t) => format!("stream<{}>", opt(t)),
        TypeDefKind::Type(t) => ty(resolve, t),
        TypeDefKind::Unknown => "unknown".into(),
    }
}

#[cfg(test)]
mod tests;
