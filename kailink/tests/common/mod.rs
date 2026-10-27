//! A session's Store for the tests: memory, table and stack pointer as kailink's
//! modules import them, and the fixtures' `kai` imports.
//!
//! The fixtures are the objects `wcc -c -nostdinc -I/kai/include` wrote for the
//! `.c` file of the same name, prefixed with `#include <kai.h>`. The compiler was
//! cc.wasm built from xcc 631e318432aeaaf72d6d2dfc93085766934314a2 (MIT), SHA-256
//! b8d5751d6b538b8bfd97b7eaf111a8587b10e3c4201f6ba0941a5e8df0f34725, on wasmi, by
//! spike S12-1 (ADR 0012; branch spike/cc-wasm, spikes/ccspike/tests/fixtures.rs).
//! The C sources are this project's.

#![allow(dead_code)]

use kailink::{FuncType, Layout, Linked, Session, ValType};
use wasmi::{
    Caller, Config, Engine, Global, Linker, Memory, MemoryType, Module, Mutability, Ref, RefType,
    Store, Table, TableType, Val,
};

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}.o", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Every fixture object, by name.
pub const FIXTURES: &[&str] = &[
    "expr",
    "define_sq",
    "use_sq",
    "global",
    "extern",
    "print",
    "struct",
    "fib",
    "fnptr",
    "need",
    "notes",
    "uses_many",
    "redefine0",
    "redefine1",
    "redefine2",
    "redefine3",
    "redefine4",
    "sum",
    "sq_i64",
    "data_ptr",
];

/// The `kai` imports the fixtures use: `print_i64`, `print_str` and `need`.
pub fn allow(field: &str, ty: &FuncType) -> bool {
    use ValType::{I32, I64};
    let want: (&[ValType], &[ValType]) = match field {
        "print_i64" => (&[I64], &[]),
        "print_str" => (&[I32, I32], &[]),
        "need" => (&[I64], &[I64]),
        _ => return false,
    };
    ty.params == want.0 && ty.results == want.1
}

pub struct Runner {
    pub session: Session,
    engine: Engine,
    store: Store<String>,
    linker: Linker<String>,
    memory: Memory,
    table: Table,
}

impl Runner {
    pub fn new() -> Self {
        let layout = Layout::default();
        let mut config = Config::default();
        config.wasm_tail_call(false);
        let engine = Engine::new(&config);
        let mut store = Store::new(&engine, String::new());
        let memory = Memory::new(&mut store, MemoryType::new(16, Some(1024))).unwrap();
        let table = Table::new(
            &mut store,
            TableType::new(RefType::Func, 1, Some(layout.table_limit)),
            Ref::null(RefType::Func),
        )
        .unwrap();
        let sp = Global::new(
            &mut store,
            Val::I32(layout.data_base as i32),
            Mutability::Var,
        );
        let mut linker = Linker::new(&engine);
        linker.define("env", "memory", memory).unwrap();
        linker.define("env", "table", table).unwrap();
        linker.define("env", "__stack_pointer", sp).unwrap();
        linker
            .func_wrap("kai", "print_i64", |mut c: Caller<String>, x: i64| {
                c.data_mut().push_str(&format!("{x}\n"));
            })
            .unwrap();
        linker
            .func_wrap(
                "kai",
                "print_str",
                move |mut c: Caller<String>, p: i32, n: i32| {
                    let (mem, out) = memory.data_and_store_mut(&mut c);
                    let bytes = &mem[p as usize..(p + n) as usize];
                    out.push_str(std::str::from_utf8(bytes).unwrap());
                },
            )
            .unwrap();
        linker
            .func_wrap("kai", "need", |_: Caller<String>, x: i64| x + 1)
            .unwrap();
        Self {
            session: Session::new(layout),
            engine,
            store,
            linker,
            memory,
            table,
        }
    }

    pub fn link(&self, name: &str) -> Result<Linked, kailink::Error> {
        self.session.link(&fixture(name), &allow)
    }

    /// Instantiates a linked module and runs `run`, without committing.
    pub fn instantiate(&mut self, linked: &Linked, run: Option<&str>) -> Option<i64> {
        let have = self.table.size(&self.store);
        let want = u64::from(linked.table_size);
        if want > have {
            self.table
                .grow(&mut self.store, want - have, Ref::null(RefType::Func))
                .unwrap();
        }
        let bytes = self.memory.data(&self.store).len() as u64;
        if u64::from(linked.data_end) > bytes {
            let pages = (u64::from(linked.data_end) - bytes).div_ceil(65536);
            self.memory.grow(&mut self.store, pages).unwrap();
        }
        let module = Module::new(&self.engine, &linked.wasm).expect("kailink's module validates");
        let instance = self
            .linker
            .instantiate_and_start(&mut self.store, &module)
            .expect("kailink's module instantiates");
        let name = run?;
        let func = instance
            .get_func(&self.store, name)
            .expect("run function exported");
        let mut out = [Val::I64(0)];
        func.call(&mut self.store, &[], &mut out).expect("runs");
        match out[0] {
            Val::I64(v) => Some(v),
            Val::I32(v) => Some(v.into()),
            _ => None,
        }
    }

    /// Links, runs and commits the fixture `name`.
    pub fn eval(&mut self, name: &str, run: Option<&str>) -> Option<i64> {
        let linked = self.link(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let result = self.instantiate(&linked, run);
        self.session.commit(&linked);
        result
    }

    pub fn output(&self) -> &str {
        self.store.data()
    }
}
