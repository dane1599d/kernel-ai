//! The compiler's Store: a fresh `cc.wasm` instance per compile, from one cached
//! module, over a [`wasihost::Wasi`] tree. Fuel is spent in slices; the run is
//! resumed after each (`ResumableCallOutOfFuel`) until wcc calls `proc_exit`.

use crate::{Clock, engine};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
use wasihost::{Limits, Stop, Tree, Wasi};
use wasmi::{
    CompilationMode, Engine, Error, Extern, FuncType, Linker, Module, ResumableCall, Store, Val,
    ValType,
};

/// What one compile cost and made.
#[derive(Debug)]
pub struct Compiled {
    pub object: Vec<u8>,
    pub ns: u64,
    pub fuel: u64,
    /// Fuel slices the run took; more than one means it was resumed.
    pub slices: u32,
    /// The compiler's linear memory at exit: its peak, since memory never shrinks.
    pub memory_bytes: usize,
}

pub struct Compiler {
    engine: Engine,
    module: Module,
    kai: Tree,
    /// The fuel of one slice.
    pub slice: u64,
}

/// The tree's `/kai/include`: xcc's headers and `kai.h`.
pub type Headers<'a> = &'a [(&'a str, &'a [u8])];

impl Compiler {
    /// Translates `cc.wasm` once; returns the compiler and the nanoseconds it took.
    pub fn new(
        wasm: &[u8],
        headers: Headers,
        mode: CompilationMode,
        clock: Clock,
    ) -> Result<(Self, u64), String> {
        let engine = Engine::new(&engine::config(mode, false));
        let start = clock();
        let module = Module::new(&engine, wasm).map_err(|e| format!("cc.wasm: {e}"))?;
        let ns = clock() - start;
        let mut kai = Tree::default();
        for (path, data) in headers {
            kai.add_kai(&format!("include/{path}"), data.to_vec())
                .map_err(|e| format!("{path}: errno {}", e.0))?;
        }
        let compiler = Self {
            engine,
            module,
            kai,
            slice: 10_000_000,
        };
        Ok((compiler, ns))
    }

    /// The same compiler with fuel slices of `slice`.
    pub fn with_slice(&self, slice: u64) -> Self {
        Self {
            engine: self.engine.clone(),
            module: self.module.clone(),
            kai: self.kai.clone(),
            slice,
        }
    }

    /// Compiles `source` as `/w/entry.c` to `/w/entry.o`; on failure, wcc's
    /// messages.
    pub fn compile(&self, source: &str, clock: Clock) -> Result<Compiled, String> {
        let start = clock();
        let mut tree = self.kai.clone();
        tree.put_scratch("entry.c", source.as_bytes())
            .map_err(|e| format!("entry.c: errno {}", e.0))?;
        let args = [
            "wcc",
            "-c",
            "-nostdinc",
            "-I/kai/include",
            "-o",
            "/w/entry.o",
            "/w/entry.c",
        ];
        let wasi =
            Wasi::new(tree, &args, Limits::default()).map_err(|e| format!("errno {}", e.0))?;
        let mut store = Store::new(&self.engine, wasi);
        let linker = self.wasi_linker()?;
        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|e| format!("instantiate cc.wasm: {e}"))?;
        let start_fn = instance
            .get_func(&store, "_start")
            .ok_or("cc.wasm has no _start")?;
        let (status, fuel, slices) = run_sliced(&mut store, start_fn, self.slice)?;
        let memory_bytes = match instance.get_export(&store, "memory") {
            Some(Extern::Memory(memory)) => memory.data(&store).len(),
            _ => 0,
        };
        let wasi = store.data();
        if status != 0 {
            let stderr = String::from_utf8_lossy(wasi.stderr()).into_owned();
            return Err(format!("wcc exited {status}: {stderr}"));
        }
        let object = wasi
            .tree()
            .read("/w/entry.o")
            .ok_or("wcc wrote no /w/entry.o")?
            .to_vec();
        Ok(Compiled {
            object,
            ns: clock() - start,
            fuel,
            slices,
            memory_bytes,
        })
    }

    /// Every import cc.wasm makes, each answered by wasihost.
    fn wasi_linker(&self) -> Result<Linker<Wasi>, String> {
        let mut linker = Linker::<Wasi>::new(&self.engine);
        for import in self.module.imports() {
            let name = import.name();
            let spec = wasihost::lookup(name)
                .filter(|_| import.module() == wasihost::MODULE)
                .ok_or_else(|| format!("cc.wasm imports {}.{name}", import.module()))?;
            let params: Vec<ValType> = spec
                .params
                .iter()
                .map(|t| match t {
                    wasihost::Ty::I32 => ValType::I32,
                    wasihost::Ty::I64 => ValType::I64,
                })
                .collect();
            let results = if spec.result {
                vec![ValType::I32]
            } else {
                vec![]
            };
            let ty = FuncType::new(params, results);
            let name_owned = name.to_string();
            linker
                .func_new(wasihost::MODULE, name, ty, move |mut caller, args, out| {
                    let memory = match caller.get_export("memory") {
                        Some(Extern::Memory(memory)) => memory,
                        _ => return Err(Error::new("no memory export")),
                    };
                    let raw: Vec<u64> = args
                        .iter()
                        .map(|v| match v {
                            Val::I32(x) => *x as u32 as u64,
                            Val::I64(x) => *x as u64,
                            _ => 0,
                        })
                        .collect();
                    let (mem, wasi) = memory.data_and_store_mut(&mut caller);
                    let result = wasi.call(&name_owned, mem, &raw);
                    #[cfg(feature = "trace")]
                    std::eprintln!("wasi {name_owned}{raw:?} -> {result:?}");
                    match result {
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

/// Runs `func` in fuel slices; returns (exit status, fuel used, slices).
fn run_sliced(
    store: &mut Store<Wasi>,
    func: wasmi::Func,
    slice: u64,
) -> Result<(i32, u64, u32), String> {
    store.set_fuel(slice).map_err(|e| e.to_string())?;
    let mut slices = 1u32;
    let mut call = func.call_resumable(&mut *store, &[], &mut []);
    loop {
        match call {
            Ok(ResumableCall::Finished) => {
                let left = store.get_fuel().unwrap_or(0);
                return Ok((0, u64::from(slices) * slice - left, slices));
            }
            Ok(ResumableCall::OutOfFuel(out)) => {
                slices += 1;
                store.set_fuel(slice).map_err(|e| e.to_string())?;
                call = out.resume(&mut *store, &mut []);
            }
            Ok(ResumableCall::HostTrap(trap)) => {
                let error = trap.into_host_error();
                let left = store.get_fuel().unwrap_or(0);
                let fuel = u64::from(slices) * slice - left;
                return match error.i32_exit_status() {
                    Some(status) => Ok((status, fuel, slices)),
                    None => Err(format!("cc.wasm host trap: {error}")),
                };
            }
            Err(error) => {
                let left = store.get_fuel().unwrap_or(0);
                let fuel = u64::from(slices) * slice - left;
                return match error.i32_exit_status() {
                    Some(status) => Ok((status, fuel, slices)),
                    None => Err(format!("cc.wasm trapped: {error}")),
                };
            }
        }
    }
}
