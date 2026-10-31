//! `cargo xtask wit-tables [--check]`: writes `wasmhost/src/tables.rs`, the host's
//! tables for the app worlds it serves, `kai:app` and `kai:ui` (ADR 0003 section 2,
//! ADR 0005 section 4): every function the worlds import and export, once each, its canonical version, its core signature and its
//! component types for `appabi`. The OS never parses WIT; this is generated on the
//! host with wit-parser, and the xtask tests fail when the file drifts from `wit/`.

use crate::wit;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use wit_parser::abi::{AbiVariant, WasmType};
use wit_parser::{Function, Handle, Resolve, Type, TypeDefKind, WorldItem};

/// Where the generated tables live.
pub const OUTPUT: &str = "wasmhost/src/tables.rs";
/// The worlds whose imports and exports the host serves, as (package, world).
const WORLDS: &[(&str, &str)] = &[
    ("kai:app", "app"),
    ("kai:ui", "app"),
    ("kai:system", "system-read"),
    ("kai:settings", "settings-access"),
    ("kai:edit", "editing"),
    ("kai:code", "coding"),
];

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let text = generate(&root.join("wit"))?;
    let path = root.join(OUTPUT);
    match args {
        [] => {
            fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("xtask: wit-tables: wrote {OUTPUT}");
            Ok(())
        }
        [flag] if flag == "--check" => check(&path, &text),
        _ => Err("usage: cargo xtask wit-tables [--check]".into()),
    }
}

/// Fails unless `path` holds exactly `text`.
pub fn check(path: &Path, text: &str) -> Result<(), String> {
    let current = fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if current != text {
        return Err(format!(
            "{OUTPUT} is out of date with wit/: run `cargo xtask wit-tables`"
        ));
    }
    Ok(())
}

/// The tables for the app worlds, as Rust source.
pub fn generate(wit_root: &Path) -> Result<String, String> {
    let (resolve, packages) = wit::load_tree(wit_root)?;
    let mut worlds = Vec::new();
    for (package, world) in WORLDS {
        let id = packages
            .iter()
            .find(|(name, _)| name.starts_with(&format!("{package}@")))
            .map(|(_, id)| *id)
            .ok_or_else(|| format!("no package {package} under wit/"))?;
        let world = resolve.packages[id]
            .worlds
            .get(*world)
            .copied()
            .ok_or_else(|| format!("{package} has no world {world}"))?;
        worlds.push(&resolve.worlds[world]);
    }
    let mut out = String::from(HEADER);
    for (list, variant) in [
        ("IMPORTS", AbiVariant::GuestImport),
        ("EXPORTS", AbiVariant::GuestExport),
    ] {
        let _ = writeln!(
            out,
            "
pub const {list}: &[WitFunc] = &["
        );
        // A world that includes another lists its interfaces again: each once.
        let mut seen = Vec::new();
        for world in &worlds {
            let items = match variant {
                AbiVariant::GuestImport => &world.imports,
                _ => &world.exports,
            };
            for item in items.values() {
                let WorldItem::Interface { id, .. } = item else {
                    continue;
                };
                if seen.contains(id) {
                    continue;
                }
                seen.push(*id);
                let iface = &resolve.interfaces[*id];
                let pkg =
                    &resolve.packages[iface.package.ok_or("an interface outside a package")?];
                let name = format!(
                    "{}:{}/{}",
                    pkg.name.namespace,
                    pkg.name.name,
                    iface.name.as_deref().unwrap_or("")
                );
                let version = pkg.name.version.as_ref().ok_or("an unversioned package")?;
                let version = canonical(version.major, version.minor);
                for func in iface.functions.values() {
                    out.push_str(&entry(&resolve, &name, &version, func, variant)?);
                }
            }
        }
        out.push_str(
            "];
",
        );
    }
    rustfmt(&out)
}

/// `text` as `cargo fmt` leaves it, so formatting the tree never makes it drift.
fn rustfmt(text: &str) -> Result<String, String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run rustfmt: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin for rustfmt")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("rustfmt failed on the generated tables".into());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.replace("\r\n", "\n"))
        .map_err(|e| e.to_string())
}

const HEADER: &str =
    "// Generated by `cargo xtask wit-tables` from wit/ (ADR 0003 section 2); do not
// edit. The xtask tests fail when it drifts from the WIT.

use crate::link::{CoreTy, WitFunc};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use appabi::Ty;
";

/// `0.minor` while 0.x, then the major: the version a host table binds.
fn canonical(major: u64, minor: u64) -> String {
    if major == 0 {
        format!("0.{minor}")
    } else {
        major.to_string()
    }
}

fn entry(
    resolve: &Resolve,
    interface: &str,
    version: &str,
    func: &Function,
    variant: AbiVariant,
) -> Result<String, String> {
    let sig = resolve.wasm_signature(variant, func);
    let core = |types: &[WasmType]| -> Result<String, String> {
        let names = types
            .iter()
            .map(|t| match t {
                WasmType::I32 | WasmType::Pointer | WasmType::Length => Ok("CoreTy::I32"),
                WasmType::I64 | WasmType::PointerOrI64 => Ok("CoreTy::I64"),
                WasmType::F32 => Ok("CoreTy::F32"),
                WasmType::F64 => Ok("CoreTy::F64"),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(format!("&[{}]", names.join(", ")))
    };
    let params = func
        .params
        .iter()
        .map(|p| ty(resolve, &p.ty))
        .collect::<Result<Vec<_>, _>>()?;
    let result = match &func.result {
        Some(t) => format!("Some({})", ty(resolve, t)?),
        None => "None".to_string(),
    };
    Ok(format!(
        "    WitFunc {{
        interface: {interface:?},
        version: {version:?},
        name: {:?},
        core_params: {},
        core_results: {},
        params: || vec![{}],
        result: || {result},
    }},
",
        func.name,
        core(&sig.params)?,
        core(&sig.results)?,
        params.join(", ")
    ))
}

/// `appabi::Ty` construction code for a WIT type.
fn ty(resolve: &Resolve, t: &Type) -> Result<String, String> {
    Ok(match t {
        Type::Bool => "Ty::Bool".into(),
        Type::U8 => "Ty::U8".into(),
        Type::U16 => "Ty::U16".into(),
        Type::U32 => "Ty::U32".into(),
        Type::U64 => "Ty::U64".into(),
        Type::S8 => "Ty::S8".into(),
        Type::S16 => "Ty::S16".into(),
        Type::S32 => "Ty::S32".into(),
        Type::S64 => "Ty::S64".into(),
        Type::F32 => "Ty::F32".into(),
        Type::F64 => "Ty::F64".into(),
        Type::Char => "Ty::Char".into(),
        Type::String => "Ty::String".into(),
        Type::Id(id) => kind(resolve, &resolve.types[*id].kind)?,
        other => return Err(format!("{other:?} is outside appabi's subset")),
    })
}

fn names<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = items.map(|n| format!("String::from({n:?})")).collect();
    format!("vec![{}]", quoted.join(", "))
}

fn kind(resolve: &Resolve, kind: &TypeDefKind) -> Result<String, String> {
    let boxed = |t: &Type| ty(resolve, t).map(|s| format!("Box::new({s})"));
    Ok(match kind {
        TypeDefKind::Type(t) => ty(resolve, t)?,
        TypeDefKind::List(t) => format!("Ty::List({})", boxed(t)?),
        TypeDefKind::Option(t) => format!("Ty::Option({})", boxed(t)?),
        TypeDefKind::Result(r) => {
            let side = |t: &Option<Type>| -> Result<String, String> {
                Ok(match t {
                    Some(t) => format!("Some({})", boxed(t)?),
                    None => "None".into(),
                })
            };
            format!("Ty::Result({}, {})", side(&r.ok)?, side(&r.err)?)
        }
        TypeDefKind::Tuple(t) => {
            let parts = t
                .types
                .iter()
                .map(|t| ty(resolve, t))
                .collect::<Result<Vec<_>, _>>()?;
            format!("Ty::Tuple(vec![{}])", parts.join(", "))
        }
        TypeDefKind::Record(r) => {
            let fields = r
                .fields
                .iter()
                .map(|f| ty(resolve, &f.ty).map(|t| format!("(String::from({:?}), {t})", f.name)))
                .collect::<Result<Vec<_>, _>>()?;
            format!("Ty::Record(vec![{}])", fields.join(", "))
        }
        TypeDefKind::Variant(v) => {
            let cases = v
                .cases
                .iter()
                .map(|c| {
                    let payload = match &c.ty {
                        Some(t) => format!("Some({})", ty(resolve, t)?),
                        None => "None".into(),
                    };
                    Ok(format!("(String::from({:?}), {payload})", c.name))
                })
                .collect::<Result<Vec<_>, String>>()?;
            format!("Ty::Variant(vec![{}])", cases.join(", "))
        }
        TypeDefKind::Enum(e) => format!(
            "Ty::Enum({})",
            names(e.cases.iter().map(|c| c.name.as_str()))
        ),
        TypeDefKind::Flags(f) => format!(
            "Ty::Flags({})",
            names(f.flags.iter().map(|c| c.name.as_str()))
        ),
        TypeDefKind::Handle(Handle::Own(_)) => "Ty::Own".into(),
        TypeDefKind::Handle(Handle::Borrow(_)) => "Ty::Borrow".into(),
        other => return Err(format!("{} is outside appabi's subset", other.as_str())),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_host_tables_match_the_wit() {
        let root = crate::workspace_root();
        let text = super::generate(&root.join("wit")).unwrap();
        super::check(&root.join(super::OUTPUT), &text).unwrap();
    }
}
