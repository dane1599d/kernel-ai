//! What `cc build` does to the module wcc links (ADR 0012 D7). Every import
//! must be one the world's glue makes, with the glue's type, so a build asks
//! for nothing its world does not: no WASI, no `env`, no other world's calls.
//! Exports take their WIT names from the world's map, since wcc reads no
//! `export_name`, and [`TRAP`]'s body becomes `unreachable`, which wcc has no
//! builtin for.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use wasmparser::{ExternalKind, FuncType, Parser, Payload, TypeRef};

/// The function `abort` ends in, whose body becomes `unreachable`: the trap a
/// `kai:app/log` panic announces.
pub const TRAP: &str = "__kai_trap";

/// `unreachable`'s body: no locals, `unreachable`, `end`.
const UNREACHABLE: [u8; 3] = [0x00, 0x00, 0x0b];

/// A function import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub ty: FuncType,
}

/// What a module imports and exports, as read by wasmparser.
#[derive(Default)]
struct Parts {
    types: Vec<FuncType>,
    imports: Vec<(String, String, TypeRef)>,
    exports: Vec<(String, ExternalKind, u32)>,
    start: bool,
}

fn parts(wasm: &[u8]) -> Result<Parts, String> {
    let mut parts = Parts::default();
    let bad = |e: wasmparser::BinaryReaderError| e.to_string();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.map_err(bad)? {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    parts.types.push(ty.map_err(bad)?);
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader {
                    let import = import.map_err(bad)?;
                    let entry = (import.module.into(), import.name.into(), import.ty);
                    parts.imports.push(entry);
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(bad)?;
                    let entry = (export.name.into(), export.kind, export.index);
                    parts.exports.push(entry);
                }
            }
            Payload::StartSection { .. } => parts.start = true,
            _ => {}
        }
    }
    Ok(parts)
}

impl Parts {
    fn func(&self, (module, name, ty): &(String, String, TypeRef)) -> Option<Import> {
        let TypeRef::Func(index) = ty else {
            return None;
        };
        Some(Import {
            module: module.clone(),
            name: name.clone(),
            ty: self.types.get(*index as usize)?.clone(),
        })
    }
}

/// The world's imports, read from its compiled glue: each function import not
/// from `env`, which only holds a relocatable object's own linker symbols.
pub fn world_imports(glue: &[u8]) -> Result<Vec<Import>, String> {
    let parts = parts(glue)?;
    Ok(parts
        .imports
        .iter()
        .filter(|(module, _, _)| module != "env")
        .filter_map(|import| parts.func(import))
        .collect())
}

/// `linked` as an artifact's module: each export renamed by `map`'s
/// `<symbol> <name>` lines, [`TRAP`]'s export dropped and its body
/// `unreachable`. Refused when it imports anything `world` lacks, exports
/// anything else but `memory`, misses an export of the map, or has a start
/// function or a `kai.` section of its own.
pub fn finish(linked: &[u8], world: &[Import], map: &str) -> Result<Vec<u8>, String> {
    let parts = parts(linked)?;
    if parts.start {
        return Err("the module has a start function".into());
    }
    let mut functions = 0u32;
    for import in &parts.imports {
        let found = parts
            .func(import)
            .ok_or_else(|| format!("{}.{} is not a function import", import.0, import.1))?;
        if !world.contains(&found) {
            return Err(format!(
                "{}.{} is not an import of the world, with its type",
                found.module, found.name
            ));
        }
        functions += 1;
    }
    let renames: Vec<(&str, &str)> = map.lines().filter_map(|l| l.split_once(' ')).collect();
    let mut trap = None;
    let mut exports = Vec::new();
    for (name, kind, index) in &parts.exports {
        match (kind, name.as_str()) {
            (ExternalKind::Memory, "memory") => exports.push((name.as_str(), *kind, *index)),
            (ExternalKind::Func, TRAP) => trap = Some(*index),
            (ExternalKind::Func, symbol) => {
                let (_, wit) = renames.iter().find(|(s, _)| *s == symbol).ok_or_else(|| {
                    format!("the module exports {symbol}, which its world does not")
                })?;
                exports.push((wit, *kind, *index));
            }
            _ => {
                return Err(format!(
                    "the module exports {name}, which its world does not"
                ));
            }
        }
    }
    for (symbol, _) in &renames {
        if !parts.exports.iter().any(|(name, _, _)| name == symbol) {
            return Err(format!("the module does not export {symbol}"));
        }
    }
    let trap = trap.ok_or("the module does not export the trap")?;
    let trap = trap.checked_sub(functions).ok_or("the trap is an import")?;
    rewrite(linked, &exports, trap)
}

/// `wasm` with its export section holding `exports` and the body of defined
/// function `trap` replaced by [`UNREACHABLE`].
fn rewrite(
    wasm: &[u8],
    exports: &[(&str, ExternalKind, u32)],
    trap: u32,
) -> Result<Vec<u8>, String> {
    let mut at = Bytes::new(wasm);
    let mut out = Vec::from(at.take(8)?);
    let mut patched = false;
    while !at.done() {
        let id = at.u8()?;
        let size = at.leb()? as usize;
        let payload = at.take(size)?;
        let new = match id {
            0 if Bytes::new(payload).name()?.starts_with("kai.") => {
                return Err("the module has a kai. section of its own".into());
            }
            7 => Some(export_section(exports)),
            10 => {
                patched = true;
                Some(code_section(payload, trap)?)
            }
            _ => None,
        };
        out.push(id);
        let body = new.as_deref().unwrap_or(payload);
        push_leb(&mut out, body.len() as u32);
        out.extend_from_slice(body);
    }
    match patched {
        true => Ok(out),
        false => Err("the module has no code".into()),
    }
}

fn export_section(exports: &[(&str, ExternalKind, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    push_leb(&mut out, exports.len() as u32);
    for (name, kind, index) in exports {
        push_leb(&mut out, name.len() as u32);
        out.extend_from_slice(name.as_bytes());
        out.push(match kind {
            ExternalKind::Func => 0,
            ExternalKind::Table => 1,
            ExternalKind::Memory => 2,
            ExternalKind::Global => 3,
            ExternalKind::Tag => 4,
        });
        push_leb(&mut out, *index);
    }
    out
}

fn code_section(payload: &[u8], trap: u32) -> Result<Vec<u8>, String> {
    let mut at = Bytes::new(payload);
    let count = at.leb()?;
    if trap >= count {
        return Err("the trap has no body".into());
    }
    let mut out = Vec::with_capacity(payload.len());
    push_leb(&mut out, count);
    for index in 0..count {
        let size = at.leb()? as usize;
        let body = at.take(size)?;
        let body = if index == trap {
            &UNREACHABLE[..]
        } else {
            body
        };
        push_leb(&mut out, body.len() as u32);
        out.extend_from_slice(body);
    }
    match at.done() {
        true => Ok(out),
        false => Err("the code section runs past its bodies".into()),
    }
}

fn push_leb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Bounds-checked reads.
struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn done(&self) -> bool {
        self.at >= self.data.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or("the module is cut short")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn leb(&mut self) -> Result<u32, String> {
        let mut value = 0u64;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return u32::try_from(value).map_err(|_| "a length is too large".to_string());
            }
        }
        Err("a length is too long".into())
    }

    fn name(&mut self) -> Result<&'a str, String> {
        let len = self.leb()? as usize;
        core::str::from_utf8(self.take(len)?).map_err(|_| "a name is not UTF-8".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leb_round_trips_and_a_long_one_is_refused() {
        for value in [0, 1, 127, 128, 300, u32::MAX] {
            let mut out = Vec::new();
            push_leb(&mut out, value);
            assert_eq!(Bytes::new(&out).leb(), Ok(value));
        }
        assert!(
            Bytes::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01])
                .leb()
                .is_err()
        );
        assert!(Bytes::new(&[0xff, 0xff, 0xff, 0xff, 0x7f]).leb().is_err());
        assert!(Bytes::new(&[0x80]).leb().is_err());
    }

    #[test]
    fn a_code_section_whose_bodies_run_short_or_long_is_refused() {
        // Two bodies of one byte each, then a stray byte.
        assert!(code_section(&[2, 1, 0x0b, 1, 0x0b, 0xff], 0).is_err());
        assert!(code_section(&[2, 1, 0x0b, 5, 0x0b], 0).is_err());
        assert!(code_section(&[1, 1, 0x0b], 1).is_err());
        assert_eq!(
            code_section(&[1, 2, 0x00, 0x0b], 0),
            Ok(alloc::vec![1, 3, 0, 0, 0x0b])
        );
    }
}
