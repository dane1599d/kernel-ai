//! The compiler's Store (ADR 0012 D3, step 3): a fresh cc.wasm instance per
//! compile, from one translated module, over a [`wasihost`] tree that holds
//! `/kai/include` read-only and the entry in `/w`. Fuel is spent in slices; the
//! budget is checked between them. Its memory may grow to [`MAX_MEMORY`].

use crate::{Budget, Stopped, engine};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
use wasihost::{Limits, Stop, Tree, Wasi};
use wasmi::{
    Engine, Error, Extern, Func, FuncType, Linker, Module, ResumableCall, Store, StoreLimits,
    StoreLimitsBuilder, Val, ValType,
};

/// Fuel per slice: a few milliseconds of work, so the deadline and Ctrl-C are
/// seen promptly.
pub const SLICE: u64 = 1_000_000;
/// Most fuel one compile may use, should the clock fail: about 13 times what
/// S12-1's 1,000-line prelude needed (73.7 M).
pub const MAX_FUEL: u64 = 1_000_000_000;
/// Most linear memory one compile may use: S12-1's pass bar. Its peak was
/// 1,152 KiB.
pub const MAX_MEMORY: usize = 32 << 20;

/// A compile's Store data: the WASI host and its memory limit.
struct Compiling {
    wasi: Wasi,
    limits: StoreLimits,
}

/// Why a compile made no object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    /// wcc's messages: the source is wrong.
    Source(String),
    Stopped(Stopped),
    /// cc.wasm itself failed: a trap, or no object.
    Compiler(String),
}

pub struct Compiler {
    engine: Engine,
    module: Module,
    kai: Tree,
}

impl Compiler {
    /// Translates `cc.wasm` once; `headers` are `/kai/include`'s files.
    pub fn new(wasm: &[u8], headers: &[(&str, &[u8])]) -> Result<Self, String> {
        let engine = Engine::new(&engine::config(false));
        let module = Module::new(&engine, wasm).map_err(|e| format!("cc.wasm: {e}"))?;
        let mut kai = Tree::default();
        for (path, data) in headers {
            kai.add_kai(&format!("include/{path}"), data.to_vec())
                .map_err(|e| format!("{path}: errno {}", e.0))?;
        }
        Ok(Self {
            engine,
            module,
            kai,
        })
    }

    /// The same translated compiler with `headers` as `/kai/include`.
    pub fn with_headers(&self, headers: &[(&str, &[u8])]) -> Result<Self, String> {
        let mut kai = Tree::default();
        for (path, data) in headers {
            kai.add_kai(&format!("include/{path}"), data.to_vec())
                .map_err(|e| format!("{path}: errno {}", e.0))?;
        }
        Ok(Self {
            engine: self.engine.clone(),
            module: self.module.clone(),
            kai,
        })
    }

    /// Compiles `source` as `/w/entry.c` to a relocatable object.
    pub fn compile(&self, source: &str, budget: &mut Budget) -> Result<Vec<u8>, CompileError> {
        let args = [
            "wcc",
            "-c",
            "-nostdinc",
            "-I/kai/include",
            "-o",
            "/w/entry.o",
            "/w/entry.c",
        ];
        self.wcc(
            &[("entry.c", source.as_bytes())],
            &args,
            "/w/entry.o",
            budget,
        )
    }

    /// Compiles `/w/<source>` to a relocatable object, with `files` (which
    /// hold it) in `/w`, so its `#include "…"` finds its neighbours.
    pub fn compile_in(
        &self,
        files: &[(&str, &[u8])],
        source: &str,
        budget: &mut Budget,
    ) -> Result<Vec<u8>, CompileError> {
        let input = format!("/w/{source}");
        let args = [
            "wcc",
            "-c",
            "-nostdinc",
            "-I/kai/include",
            "-o",
            "/w/out.o",
            &input,
        ];
        self.wcc(files, &args, "/w/out.o", budget)
    }

    /// Links `objects`, each put in `/w` by its name, into a module with no
    /// entry point and no libraries that exports `exports` by symbol name, with
    /// a `stack`-byte stack. A symbol nothing defines is an error that names it.
    pub fn link(
        &self,
        objects: &[(&str, &[u8])],
        exports: &[&str],
        stack: u32,
        budget: &mut Budget,
    ) -> Result<Vec<u8>, CompileError> {
        let exports = exports.join(",");
        let stack = format!("--stack-size={stack}");
        let paths: Vec<String> = objects.iter().map(|(n, _)| format!("/w/{n}")).collect();
        let mut args = vec![
            "wcc",
            "-nostdlib",
            "--entry-point=",
            &stack,
            "-e",
            &exports,
            "-o",
            "/w/out.wasm",
        ];
        args.extend(paths.iter().map(String::as_str));
        self.wcc(objects, &args, "/w/out.wasm", budget)
    }

    /// Runs wcc with `args` over a tree holding `/kai` and `files` in `/w`;
    /// the file it wrote at `output`.
    fn wcc(
        &self,
        files: &[(&str, &[u8])],
        args: &[&str],
        output: &str,
        budget: &mut Budget,
    ) -> Result<Vec<u8>, CompileError> {
        let fail = |e: String| CompileError::Compiler(e);
        let mut tree = self.kai.clone();
        for (name, data) in files {
            tree.put_scratch(name, data)
                .map_err(|e| fail(format!("{name}: errno {}", e.0)))?;
        }
        let wasi =
            Wasi::new(tree, args, Limits::default()).map_err(|e| fail(format!("errno {}", e.0)))?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY)
            .instances(1)
            .build();
        let mut store = Store::new(&self.engine, Compiling { wasi, limits });
        store.limiter(|data| &mut data.limits);
        let linker = self.linker().map_err(fail)?;
        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|e| fail(format!("instantiate cc.wasm: {e}")))?;
        let start = instance
            .get_func(&store, "_start")
            .ok_or_else(|| fail("cc.wasm has no _start".into()))?;
        let status = run(&mut store, start, budget)?;
        let wasi = &store.data().wasi;
        if status != 0 {
            let stderr = String::from_utf8_lossy(wasi.stderr()).into_owned();
            return Err(CompileError::Source(stderr));
        }
        match wasi.tree().read(output) {
            Some(out) => Ok(out.to_vec()),
            None => Err(fail(format!("wcc wrote no {output}"))),
        }
    }

    /// Every import cc.wasm makes, each answered by wasihost.
    fn linker(&self) -> Result<Linker<Compiling>, String> {
        let mut linker = Linker::<Compiling>::new(&self.engine);
        for import in self.module.imports() {
            let name = import.name();
            let spec = wasihost::lookup(name)
                .filter(|_| import.module() == wasihost::MODULE)
                .ok_or_else(|| format!("cc.wasm imports {}.{name}", import.module()))?;
            let params = spec.params.iter().map(|t| match t {
                wasihost::Ty::I32 => ValType::I32,
                wasihost::Ty::I64 => ValType::I64,
            });
            let results = if spec.result {
                vec![ValType::I32]
            } else {
                vec![]
            };
            let ty = FuncType::new(params, results);
            let owned = name.to_string();
            linker
                .func_new(wasihost::MODULE, name, ty, move |mut caller, args, out| {
                    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                        return Err(Error::new("cc.wasm exports no memory"));
                    };
                    let raw: Vec<u64> = args
                        .iter()
                        .map(|v| match v {
                            Val::I32(x) => *x as u32 as u64,
                            Val::I64(x) => *x as u64,
                            _ => 0,
                        })
                        .collect();
                    let (mem, data) = memory.data_and_store_mut(&mut caller);
                    match data.wasi.call(&owned, mem, &raw) {
                        Ok(errno) => {
                            if let Some(slot) = out.first_mut() {
                                *slot = Val::I32(i32::from(errno));
                            }
                            Ok(())
                        }
                        Err(Stop::Exit(code)) => Err(Error::i32_exit(code as i32)),
                        Err(Stop::BadCall) => Err(Error::new("bad WASI call")),
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(linker)
    }
}

/// Runs `_start` in fuel slices; wcc's exit status.
fn run(
    store: &mut Store<Compiling>,
    start: Func,
    budget: &mut Budget,
) -> Result<i32, CompileError> {
    let exit = |error: &Error| match error.i32_exit_status() {
        Some(status) => Ok(status),
        None => Err(CompileError::Compiler(format!("cc.wasm trapped: {error}"))),
    };
    let mut used = 0u64;
    store
        .set_fuel(SLICE)
        .map_err(|e| CompileError::Compiler(e.to_string()))?;
    let mut call = start.call_resumable(&mut *store, &[], &mut []);
    loop {
        match call {
            Ok(ResumableCall::Finished) => return Ok(0),
            Ok(ResumableCall::OutOfFuel(out)) => {
                used += SLICE;
                if used >= MAX_FUEL {
                    return Err(CompileError::Compiler(
                        "the compile used too much fuel".into(),
                    ));
                }
                budget.check().map_err(CompileError::Stopped)?;
                store
                    .set_fuel(SLICE)
                    .map_err(|e| CompileError::Compiler(e.to_string()))?;
                call = out.resume(&mut *store, &mut []);
            }
            Ok(ResumableCall::HostTrap(trap)) => return exit(trap.host_error()),
            Err(error) => return exit(&error),
        }
    }
}
