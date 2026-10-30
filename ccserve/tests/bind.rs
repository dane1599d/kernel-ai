//! `cc build`'s bindings (ADR 0012 D7) compile with the pinned cc.wasm, with
//! their imports named as the OS serves them: S12-1's question (h).

use ccserve::bind::{self, World};
use ccserve::{Budget, Compiler, Host, XCC_HEADERS};

struct Clock;

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        0
    }

    fn interrupted(&self) -> bool {
        false
    }
}

/// The world's glue compiled against its header: the object.
fn compile(world: &World) -> Vec<u8> {
    let mut headers: Vec<(&str, &[u8])> = XCC_HEADERS.to_vec();
    headers.push(("stdlib.h", bind::STDLIB_H));
    headers.push((world.header_name, world.header));
    let compiler = Compiler::new(ccserve::CC_WASM, &headers).expect("cc.wasm");
    let mut budget = Budget::new(&Clock, u64::MAX / 4);
    compiler
        .compile(world.glue, &mut budget)
        .unwrap_or_else(|e| panic!("{}: {e:?}", world.header_name))
}

fn holds(object: &[u8], text: &str) -> bool {
    object.windows(text.len()).any(|w| w == text.as_bytes())
}

fn leb(bytes: &[u8], at: &mut usize) -> u32 {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = bytes[*at];
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    value
}

fn name(bytes: &[u8], at: &mut usize) -> String {
    let len = leb(bytes, at) as usize;
    let text = String::from_utf8(bytes[*at..*at + len].to_vec()).expect("UTF-8");
    *at += len;
    text
}

/// The object's function imports, as (module, name).
fn function_imports(object: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at = 8;
    while at < object.len() {
        let id = object[at];
        at += 1;
        let size = leb(object, &mut at) as usize;
        let end = at + size;
        if id == 2 {
            let mut p = at;
            for _ in 0..leb(object, &mut p) {
                let module = name(object, &mut p);
                let field = name(object, &mut p);
                let kind = object[p];
                p += 1;
                match kind {
                    0 => {
                        leb(object, &mut p);
                        out.push((module, field));
                    }
                    // A table: its type and limits.
                    1 => {
                        p += 1;
                        let flags = object[p];
                        p += 1;
                        leb(object, &mut p);
                        if flags & 1 != 0 {
                            leb(object, &mut p);
                        }
                    }
                    2 => {
                        let flags = object[p];
                        p += 1;
                        leb(object, &mut p);
                        if flags & 1 != 0 {
                            leb(object, &mut p);
                        }
                    }
                    // A global: its value type and mutability.
                    _ => p += 2,
                }
            }
        }
        at = end;
    }
    out
}

#[test]
fn each_worlds_glue_compiles_with_its_imports_named() {
    for (world, modules) in [
        (&bind::COMPONENT, &["kai:app/log@0.1.0"][..]),
        (
            &bind::APP,
            &[
                "kai:app/log@0.1.0",
                "kai:app/clock@0.1.0",
                "kai:app/storage@0.1.0",
                "kai:app/effects@0.1.0",
            ][..],
        ),
    ] {
        let imports = function_imports(&compile(world));
        for module in modules {
            assert!(
                imports.iter().any(|(m, _)| m == module),
                "{}: no import from {module} in {imports:?}",
                world.header_name
            );
        }
        // Not as `env`'s C names, which no host serves.
        assert!(
            !imports.iter().any(|(_, n)| n.starts_with("__wasm_import_")),
            "{}: {imports:?}",
            world.header_name
        );
    }
    assert!(
        function_imports(&compile(&bind::COMPONENT))
            .contains(&(String::from("kai:app/log@0.1.0"), String::from("write")))
    );
}

#[test]
fn every_export_in_the_map_is_defined_in_the_glue() {
    for world in [&bind::COMPONENT, &bind::APP] {
        let object = compile(world);
        let lines: Vec<&str> = world.exports.lines().collect();
        assert!(lines.len() >= 8, "{}: {lines:?}", world.header_name);
        for line in lines {
            let (symbol, name) = line.split_once(' ').expect("<symbol> <name>");
            assert!(holds(&object, symbol), "{}: {symbol}", world.header_name);
            assert!(!name.is_empty() && !name.contains(' '));
        }
    }
}
