//! `:dis f` (ADR 0012 D4, TempleOS's `Uf`): the code of `f` as Wasm text, one
//! instruction a line, indented by block. A small printer over wasmparser: an
//! operator's name comes from its variant (`I64ExtendI32S` is
//! `i64.extend_i32_s`), its common operands are written out, and a call names
//! its callee: an import by its field, an entry's function by its symbol, and a
//! thunk (a call into an earlier entry, D3 step 4) by the symbol of the slot it
//! calls through.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use wasmparser::{ExternalKind, FunctionBody, Operator, Parser, Payload, TypeRef};

/// Longest listing, so it fits in one reply.
pub const MAX_TEXT: usize = 6 << 10;

/// The listing of the function `name` exports from `wasm`; `slot_name` names
/// a table slot. `None` if `wasm` does not export `name` or does not parse.
pub fn function(
    wasm: &[u8],
    name: &str,
    slot_name: &dyn Fn(u32) -> Option<String>,
) -> Option<String> {
    let mut imports: Vec<String> = Vec::new();
    let mut exports: Vec<(String, u32)> = Vec::new();
    let mut bodies: Vec<FunctionBody> = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.ok()? {
            Payload::ImportSection(reader) => {
                for import in reader {
                    let import = import.ok()?;
                    if matches!(import.ty, TypeRef::Func(_)) {
                        imports.push(import.name.to_string());
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.ok()?;
                    if export.kind == ExternalKind::Func {
                        exports.push((export.name.to_string(), export.index));
                    }
                }
            }
            Payload::CodeSectionEntry(body) => bodies.push(body),
            _ => {}
        }
    }
    let index = exports.iter().find(|(n, _)| n == name)?.1;
    let body = bodies.get((index as usize).checked_sub(imports.len())?)?;
    let callee = |index: u32| -> String {
        let i = index as usize;
        if let Some(field) = imports.get(i) {
            return field.clone();
        }
        if let Some((n, _)) = exports.iter().find(|(_, e)| *e == index) {
            return n.clone();
        }
        let thunk = bodies.get(i - imports.len()).and_then(thunk_slot);
        match thunk.and_then(|slot| slot_name(slot).map(|n| (slot, n))) {
            Some((slot, n)) => format!("{n} (slot {slot})"),
            None => format!("${index}"),
        }
    };
    render(name, body, &callee)
}

/// The slot a thunk calls through: its body is `local.get`s, `i32.const slot`,
/// `call_indirect`, `end`.
fn thunk_slot(body: &FunctionBody) -> Option<u32> {
    let mut ops = body.get_operators_reader().ok()?;
    let mut slot = None;
    while !ops.eof() {
        match ops.read().ok()? {
            Operator::LocalGet { .. } => {}
            Operator::I32Const { value } if slot.is_none() => slot = Some(value as u32),
            Operator::CallIndirect { .. } if slot.is_some() => {
                return match ops.read().ok()? {
                    Operator::End if ops.eof() => slot,
                    _ => None,
                };
            }
            _ => return None,
        }
    }
    None
}

fn render(name: &str, body: &FunctionBody, callee: &dyn Fn(u32) -> String) -> Option<String> {
    let mut out = String::new();
    let _ = writeln!(out, "func {name}");
    let mut locals = body.get_locals_reader().ok()?;
    for _ in 0..locals.get_count() {
        let (count, ty) = locals.read().ok()?;
        let _ = writeln!(
            out,
            "  (local {count} {})",
            format!("{ty:?}").to_lowercase()
        );
    }
    let mut ops = body.get_operators_reader().ok()?;
    let mut depth = 1usize;
    while !ops.eof() {
        let op = ops.read().ok()?;
        if matches!(op, Operator::End | Operator::Else) {
            depth = depth.saturating_sub(1);
        }
        if ops.eof() {
            // The function's own `end`.
            break;
        }
        let _ = writeln!(out, "{:width$}{}", "", text(&op, callee), width = 2 * depth);
        if matches!(
            op,
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } | Operator::Else
        ) {
            depth += 1;
        }
        if out.len() > MAX_TEXT {
            out.truncate(MAX_TEXT);
            out.push_str("\n…\n");
            break;
        }
    }
    Some(out)
}

/// One operator as Wasm text.
fn text(op: &Operator, callee: &dyn Fn(u32) -> String) -> String {
    let debug = format!("{op:?}");
    let variant = debug.split([' ', '{', '(']).next().unwrap_or("");
    let name = wasm_name(variant);
    let operand = match op {
        Operator::LocalGet { local_index }
        | Operator::LocalSet { local_index }
        | Operator::LocalTee { local_index } => format!(" {local_index}"),
        Operator::GlobalGet { global_index } | Operator::GlobalSet { global_index } => {
            match global_index {
                // kailink's modules import one global: the stack pointer.
                0 => " $__stack_pointer".into(),
                i => format!(" {i}"),
            }
        }
        Operator::I32Const { value } => format!(" {value}"),
        Operator::I64Const { value } => format!(" {value}"),
        Operator::F32Const { value } => format!(" {}", f32::from_bits(value.bits())),
        Operator::F64Const { value } => format!(" {}", f64::from_bits(value.bits())),
        Operator::Call { function_index } => format!(" {}", callee(*function_index)),
        Operator::CallIndirect { type_index, .. } => format!(" (type {type_index})"),
        Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => {
            format!(" {relative_depth}")
        }
        Operator::BrTable { targets } => {
            let mut list: Vec<String> = targets
                .targets()
                .filter_map(Result::ok)
                .map(|t| format!("{t}"))
                .collect();
            list.push(format!("{}", targets.default()));
            format!(" {}", list.join(" "))
        }
        _ => memarg_offset(&debug),
    };
    format!("{name}{operand}")
}

/// A load's or store's ` offset=N`, read from its Debug form; "" for others.
fn memarg_offset(debug: &str) -> String {
    let Some(at) = debug.find("offset: ") else {
        return String::new();
    };
    let digits: String = debug[at + 8..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.as_str() {
        "" | "0" => String::new(),
        n => format!(" offset={n}"),
    }
}

/// `I64ExtendI32S` → `i64.extend_i32_s`, `LocalGet` → `local.get`, `BrIf` →
/// `br_if`.
fn wasm_name(variant: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for c in variant.chars() {
        match words.last_mut() {
            Some(word) if !c.is_ascii_uppercase() => word.push(c),
            _ => words.push(c.to_string()),
        }
    }
    let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    match lower.split_first() {
        Some((first, rest)) if !rest.is_empty() && dotted(first) => {
            format!("{first}.{}", rest.join("_"))
        }
        _ => lower.join("_"),
    }
}

fn dotted(word: &str) -> bool {
    matches!(
        word,
        "i32"
            | "i64"
            | "f32"
            | "f64"
            | "v128"
            | "local"
            | "global"
            | "memory"
            | "table"
            | "ref"
            | "elem"
            | "data"
    )
}

#[cfg(test)]
mod tests {
    use super::wasm_name;

    #[test]
    fn variants_become_wasm_names() {
        for (variant, name) in [
            ("I64ExtendI32S", "i64.extend_i32_s"),
            ("I32Add", "i32.add"),
            ("LocalGet", "local.get"),
            ("GlobalSet", "global.set"),
            ("MemoryGrow", "memory.grow"),
            ("BrIf", "br_if"),
            ("CallIndirect", "call_indirect"),
            ("Return", "return"),
            ("I32TruncF64S", "i32.trunc_f64_s"),
            ("Unreachable", "unreachable"),
        ] {
            assert_eq!(wasm_name(variant), name);
        }
    }
}
