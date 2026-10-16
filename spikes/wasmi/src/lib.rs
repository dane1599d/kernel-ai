//! M0 spike: instantiate and call a WebAssembly module with no standard library.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use wasmi::{Engine, Linker, Module, Store};

/// A hand-assembled module exporting `add(i32, i32) -> i32`.
const ADD_MODULE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version 1
    0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f, // type: (i32, i32) -> i32
    0x03, 0x02, 0x01, 0x00, // function 0 has type 0
    0x07, 0x07, 0x01, 0x03, b'a', b'd', b'd', 0x00, 0x00, // export "add" = function 0
    0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a,
    0x0b, // body: local.get 0, local.get 1, i32.add
];

/// Instantiates the module and calls `add(40, 2)`. Expected result: 42.
pub fn run() -> Result<i32, String> {
    let engine = Engine::default();
    let module = Module::new(&engine, ADD_MODULE).map_err(|e| e.to_string())?;
    let mut store = Store::new(&engine, ());
    let linker = Linker::<()>::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let add = instance
        .get_typed_func::<(i32, i32), i32>(&store, "add")
        .map_err(|e| e.to_string())?;
    add.call(&mut store, (40, 2)).map_err(|e| e.to_string())
}
