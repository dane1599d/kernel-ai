//! [`finish`] on hand-built modules: every refusal, which honest wcc output
//! never reaches, and the rewrite itself.

use super::*;
use alloc::vec;
use wasmparser::ValType;

const LOG: &str = "kai:app/log@0.1.0";
const MAP: &str = "sym_a a#x\n";

/// A module whose functions all take and return nothing.
#[derive(Default)]
struct Module<'a> {
    /// (module, name, the import's kind and type bytes).
    imports: Vec<(&'a str, &'a str, Vec<u8>)>,
    functions: u32,
    table: bool,
    memory: bool,
    global: bool,
    exports: Vec<(&'a str, u8, u32)>,
    start: Option<u32>,
    custom: Option<&'a str>,
}

const FUNC: [u8; 2] = [0x00, 0x00];
const MEMORY: [u8; 3] = [0x02, 0x00, 0x01];

fn section(out: &mut Vec<u8>, id: u8, payload: &[u8]) {
    out.push(id);
    push_leb(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

fn name(out: &mut Vec<u8>, text: &str) {
    push_leb(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

impl Module<'_> {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::from(&b"\0asm\x01\0\0\0"[..]);
        section(&mut out, 1, &[1, 0x60, 0, 0]);
        let mut imports = vec![self.imports.len() as u8];
        for (module, field, kind) in &self.imports {
            name(&mut imports, module);
            name(&mut imports, field);
            imports.extend_from_slice(kind);
        }
        section(&mut out, 2, &imports);
        let mut functions = vec![self.functions as u8];
        functions.extend(core::iter::repeat_n(0, self.functions as usize));
        section(&mut out, 3, &functions);
        if self.table {
            section(&mut out, 4, &[1, 0x70, 0x00, 0x01]);
        }
        if self.memory {
            section(&mut out, 5, &[1, 0x00, 0x01]);
        }
        if self.global {
            section(&mut out, 6, &[1, 0x7f, 0x00, 0x41, 0x00, 0x0b]);
        }
        let mut exports = vec![self.exports.len() as u8];
        for (field, kind, index) in &self.exports {
            name(&mut exports, field);
            exports.push(*kind);
            push_leb(&mut exports, *index);
        }
        section(&mut out, 7, &exports);
        if let Some(start) = self.start {
            section(&mut out, 8, &[start as u8]);
        }
        let mut code = vec![self.functions as u8];
        for _ in 0..self.functions {
            code.extend_from_slice(&[2, 0x00, 0x0b]);
        }
        section(&mut out, 10, &code);
        if let Some(custom) = self.custom {
            let mut payload = Vec::new();
            name(&mut payload, custom);
            payload.extend_from_slice(b"data");
            section(&mut out, 0, &payload);
        }
        out
    }
}

/// One import from `kai:app/log`, a defined `sym_a`, the trap and `memory`.
fn honest() -> Module<'static> {
    Module {
        imports: vec![(LOG, "write", FUNC.to_vec())],
        functions: 2,
        memory: true,
        exports: vec![("sym_a", 0, 1), (TRAP, 0, 2), ("memory", 2, 0)],
        ..Module::default()
    }
}

fn world() -> Vec<Import> {
    vec![Import {
        module: LOG.into(),
        name: "write".into(),
        ty: FuncType::new([], []),
    }]
}

fn refused(module: &Module, map: &str) -> String {
    finish(&module.bytes(), &world(), map).unwrap_err()
}

#[test]
fn an_honest_module_takes_its_names_and_the_trap_becomes_unreachable() {
    let out = finish(&honest().bytes(), &world(), MAP).unwrap();
    let mut exports = Vec::new();
    let mut bodies = Vec::new();
    for payload in Parser::new(0).parse_all(&out) {
        match payload.unwrap() {
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.unwrap();
                    exports.push((export.name.to_string(), export.index));
                }
            }
            Payload::CodeSectionEntry(body) => bodies.push(out[body.range()].to_vec()),
            _ => {}
        }
    }
    assert_eq!(exports, [("a#x".into(), 1), ("memory".into(), 0)]);
    assert_eq!(bodies, [vec![0x00, 0x0b], UNREACHABLE.to_vec()]);
}

#[test]
fn a_start_function_is_refused() {
    let module = Module {
        start: Some(1),
        ..honest()
    };
    assert!(refused(&module, MAP).contains("start function"));
}

#[test]
fn imports_outside_the_world_are_refused() {
    let mut memory = honest();
    memory.imports.push(("env", "memory", MEMORY.to_vec()));
    assert!(refused(&memory, MAP).contains("not a function import"));
    let mut wasi = honest();
    wasi.imports = vec![("wasi_snapshot_preview1", "fd_write", FUNC.to_vec())];
    assert!(refused(&wasi, MAP).contains("wasi_snapshot_preview1.fd_write"));
    let other_type = vec![Import {
        ty: FuncType::new([ValType::I32], []),
        ..world().remove(0)
    }];
    assert!(finish(&honest().bytes(), &other_type, MAP).is_err());
}

#[test]
fn exports_outside_the_map_are_refused() {
    for (field, kind, index) in [("stray", 0, 1), ("t", 1, 0), ("g", 3, 0), ("mem", 2, 0)] {
        let mut module = Module {
            table: true,
            global: true,
            ..honest()
        };
        module.exports.push((field, kind, index));
        assert!(refused(&module, MAP).contains(field), "{field}");
    }
    assert!(refused(&honest(), "sym_a a#x\nsym_b b#y\n").contains("sym_b"));
    let mut no_trap = honest();
    no_trap.exports.retain(|(field, _, _)| *field != TRAP);
    assert!(refused(&no_trap, MAP).contains("trap"));
}

#[test]
fn a_trap_that_is_an_import_is_refused() {
    let mut module = honest();
    module.exports[1] = (TRAP, 0, 0);
    assert!(refused(&module, MAP).contains("the trap is an import"));
}

#[test]
fn a_kai_section_of_its_own_is_refused() {
    let module = Module {
        custom: Some("kai.manifest"),
        ..honest()
    };
    assert!(refused(&module, MAP).contains("kai."));
    let other = Module {
        custom: Some("name"),
        ..honest()
    };
    assert!(finish(&other.bytes(), &world(), MAP).is_ok());
}

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
        Ok(vec![1, 3, 0, 0, 0x0b])
    );
}
