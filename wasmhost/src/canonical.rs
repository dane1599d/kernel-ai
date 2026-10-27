//! A `lifecycle 2` version (ADR 0003 sections 2-3): a module on the canonical ABI,
//! reached through `appabi`. Its `kai:*` imports are linked by canonical version
//! from the generated tables: `kai:app/log` and `kai:app/clock` now, `storage` and
//! `effects` with the effect transaction. Its `kai:live/lifecycle` exports serve
//! `livecore::Instance`; app exports are reached with `Instance::call`. Each call
//! gets fresh fuel, and time and the seed are fixed for the call.

use crate::instance::{HostState, classify, fault, log, panic};
use crate::link::{self, WitFunc};
use crate::{Limits, LogSink, config, tables};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Trap, Val};
use kabi::live::MAX_CHECKPOINT;
use livecore::{Fault, Reason};
use wasmi::errors::ErrorKind;
use wasmi::{
    Caller, Engine, Error, ExternType, Linker, Memory, Module, Store, StoreLimitsBuilder, TypedFunc,
};

/// An instance of a canonical-ABI module.
pub struct CoreInstance {
    store: Store<HostState>,
    instance: wasmi::Instance,
    memory: Memory,
    realloc: Option<TypedFunc<(i32, i32, i32, i32), i32>>,
    limits: Limits,
    /// Fuel for each core call of the call in progress.
    fuel: u64,
    /// The fault behind the last failed core call, which `appabi` sees as a trap.
    last: Option<Fault>,
    /// The `kai:live/lifecycle` exports, by name, when the module has them.
    lifecycle: Vec<(&'static str, String)>,
}

fn link_err(detail: impl Into<String>) -> Fault {
    Fault::new(Reason::Link, detail)
}

/// Compiles, links and instantiates a canonical-ABI module. `log` receives its
/// `kai:app/log` lines.
#[inline(never)]
pub fn load_core(
    wasm: &[u8],
    limits: Limits,
    log_sink: Option<LogSink>,
) -> Result<CoreInstance, Fault> {
    if wasm.len() > kabi::live::MAX_ARTIFACT {
        return Err(Fault::new(Reason::Compile, "the module is too large"));
    }
    let engine = Engine::new(&config());
    let module = Module::new(&engine, wasm)
        .map_err(|error| Fault::new(Reason::Compile, error.to_string()))?;
    let linker = link_imports(&engine, &module)?;
    let state = HostState {
        limits: StoreLimitsBuilder::new()
            .memory_size(limits.memory_bytes)
            .memories(1)
            .tables(1)
            .table_elements(4096)
            .instances(1)
            .trap_on_grow_failure(true)
            .build(),
        log: log_sink,
        quiet: false,
        lines: 0,
        panic_at: None,
        now_ms: 0,
        seed: 0,
    };
    let mut store = Store::new(&engine, state);
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(limits.fuel_life)
        .map_err(|error| Fault::new(Reason::Host, error.to_string()))?;
    let instance =
        linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|error| match error.kind() {
                ErrorKind::Linker(_) | ErrorKind::Instantiation(_) => link_err(error.to_string()),
                _ => classify(&error),
            })?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| Fault::new(Reason::Abi, "memory is not exported"))?;
    let realloc = instance.get_typed_func(&store, "cabi_realloc").ok();
    let lifecycle = lifecycle_exports(&module);
    Ok(CoreInstance {
        store,
        instance,
        memory,
        realloc,
        limits,
        fuel: limits.fuel_call,
        last: None,
        lifecycle,
    })
}

/// Binds every import the module makes, or fails naming the first it cannot.
fn link_imports(engine: &Engine, module: &Module) -> Result<Linker<HostState>, Fault> {
    let mut linker = Linker::<HostState>::new(engine);
    for import in module.imports() {
        let (qualified, name) = (import.module(), import.name());
        let ExternType::Func(ty) = import.ty() else {
            return Err(link_err(format!("{qualified} {name} is not a function")));
        };
        let entry = link::find(tables::IMPORTS, qualified, name)
            .ok_or_else(|| link_err(format!("no host function {qualified} {name}")))?;
        if !link::core_matches(entry.core_params, ty.params())
            || !link::core_matches(entry.core_results, ty.results())
        {
            return Err(link_err(format!("{qualified} {name} has the wrong type")));
        }
        let bound = match (entry.interface, entry.name) {
            ("kai:app/log", "write") => linker.func_wrap(qualified, name, log),
            ("kai:app/log", "panic") => linker.func_wrap(qualified, name, panic),
            ("kai:app/clock", "now-ms") => {
                linker.func_wrap(qualified, name, |c: Caller<'_, HostState>| {
                    c.data().now_ms as i64
                })
            }
            ("kai:app/clock", "seed") => {
                linker.func_wrap(qualified, name, |c: Caller<'_, HostState>| {
                    c.data().seed as i64
                })
            }
            (interface, _) => {
                return Err(link_err(format!(
                    "{interface} is not offered by this host yet"
                )));
            }
        };
        bound.map_err(|error| Fault::new(Reason::Host, error.to_string()))?;
    }
    Ok(linker)
}

/// The module's export for `interface`'s `name` at a version that links.
fn export_for(module: &Module, entry: &WitFunc) -> Option<String> {
    module.exports().find_map(|export| {
        let (qualified, name) = export.name().split_once('#')?;
        let (interface, version) = link::split(qualified)?;
        (interface == entry.interface && version == entry.version && name == entry.name)
            .then(|| export.name().to_string())
    })
}

fn lifecycle_exports(module: &Module) -> Vec<(&'static str, String)> {
    tables::EXPORTS
        .iter()
        .filter(|f| f.interface == "kai:live/lifecycle")
        .filter_map(|f| export_for(module, f).map(|export| (f.name, export)))
        .collect()
}

impl CoreInstance {
    /// Fixes the clock and seed the next calls see.
    pub fn set_call_inputs(&mut self, now_ms: u64, seed: u64) {
        let state = self.store.data_mut();
        state.now_ms = now_ms;
        state.seed = seed;
    }

    /// Calls `func` through `appabi` with `fuel` for each core call.
    fn typed(
        &mut self,
        func: &appabi::Func,
        args: &[Val],
        fuel: u64,
    ) -> Result<Option<Val>, Fault> {
        self.fuel = fuel;
        self.last = None;
        let state = self.store.data_mut();
        state.lines = 0;
        state.panic_at = None;
        appabi::call(self, func, args).map_err(|trap| {
            self.last
                .take()
                .unwrap_or_else(|| Fault::new(Reason::Abi, trap.0))
        })
    }

    /// Calls lifecycle function `name`.
    fn lifecycle(&mut self, name: &str, args: &[Val]) -> Result<Option<Val>, Fault> {
        let export = self
            .lifecycle
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, e)| e.clone())
            .ok_or_else(|| {
                Fault::new(
                    Reason::Abi,
                    format!("kai:live/lifecycle {name} is not exported"),
                )
            })?;
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == "kai:live/lifecycle" && f.name == name)
            .ok_or_else(|| Fault::new(Reason::Host, "no lifecycle table"))?;
        let func = entry.func(export);
        self.typed(&func, args, self.limits.fuel_life)
    }

    fn code(&mut self, name: &str) -> Result<u32, Fault> {
        match self.lifecycle(name, &[])? {
            Some(Val::U32(code)) => Ok(code),
            other => Err(Fault::new(
                Reason::Abi,
                format!("{name} returned {other:?}"),
            )),
        }
    }

    /// 0, or 1 + the `start-error` case.
    fn start(result: Option<Val>, name: &str) -> Result<u32, Fault> {
        match result {
            Some(Val::Result(Ok(None))) => Ok(0),
            Some(Val::Result(Err(Some(error)))) => match *error {
                Val::Enum(case) => Ok(case + 1),
                other => Err(Fault::new(
                    Reason::Abi,
                    format!("{name} failed with {other:?}"),
                )),
            },
            other => Err(Fault::new(
                Reason::Abi,
                format!("{name} returned {other:?}"),
            )),
        }
    }
}

impl Guest for CoreInstance {
    #[inline(never)]
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let data = self.memory.data(&self.store);
        let start = at as usize;
        let end = start
            .checked_add(len as usize)
            .ok_or_else(|| Trap("read past memory".into()))?;
        data.get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Trap("read past memory".into()))
    }

    #[inline(never)]
    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        self.memory
            .write(&mut self.store, at as usize, bytes)
            .map_err(|error| Trap(error.to_string()))
    }

    #[inline(never)]
    fn realloc(&mut self, old: u32, old_size: u32, align: u32, size: u32) -> Result<u32, Trap> {
        let realloc = self
            .realloc
            .ok_or_else(|| Trap("no cabi_realloc export".into()))?;
        self.store
            .set_fuel(self.fuel)
            .map_err(|error| Trap(error.to_string()))?;
        let args = (old as i32, old_size as i32, align as i32, size as i32);
        realloc
            .call(&mut self.store, args)
            .map(|at| at as u32)
            .map_err(|error| self.failed(&error))
    }

    #[inline(never)]
    fn call(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap> {
        let func = self
            .instance
            .get_func(&self.store, export)
            .ok_or_else(|| Trap(format!("no export {export}")))?;
        let inputs: Vec<wasmi::Val> = args
            .iter()
            .map(|arg| match *arg {
                Core::I32(v) => wasmi::Val::I32(v),
                Core::I64(v) => wasmi::Val::I64(v),
                Core::F32(v) => wasmi::Val::F32(v.into()),
                Core::F64(v) => wasmi::Val::F64(v.into()),
            })
            .collect();
        let mut outputs = vec![wasmi::Val::I32(0); func.ty(&self.store).results().len()];
        self.store
            .set_fuel(self.fuel)
            .map_err(|error| Trap(error.to_string()))?;
        func.call(&mut self.store, &inputs, &mut outputs)
            .map_err(|error| self.failed(&error))?;
        outputs
            .into_iter()
            .map(|value| match value {
                wasmi::Val::I32(v) => Ok(Core::I32(v)),
                wasmi::Val::I64(v) => Ok(Core::I64(v)),
                wasmi::Val::F32(v) => Ok(Core::F32(v.into())),
                wasmi::Val::F64(v) => Ok(Core::F64(v.into())),
                _ => Err(Trap("a result outside the core types".into())),
            })
            .collect()
    }

    fn rep(&mut self, _h: u32) -> Result<u32, Trap> {
        Err(Trap("this host has no resources yet".into()))
    }
}

impl CoreInstance {
    /// Keeps a failed core call's fault for the caller, and gives `appabi` a trap.
    fn failed(&mut self, error: &Error) -> Trap {
        let panic_at = self.store.data_mut().panic_at.take();
        let fault = fault(error, panic_at);
        let trap = Trap(fault.detail.clone());
        self.last = Some(fault);
        trap
    }
}

impl livecore::Instance for CoreInstance {
    #[inline(never)]
    fn schema(&mut self) -> Result<u32, Fault> {
        match self.code("schema")? {
            0 => Err(Fault::new(Reason::Abi, "schema returned 0")),
            schema => Ok(schema),
        }
    }

    #[inline(never)]
    fn init(&mut self) -> Result<u32, Fault> {
        let result = self.lifecycle("init", &[])?;
        Self::start(result, "init")
    }

    #[inline(never)]
    fn import(&mut self, state: &[u8], from_schema: u32) -> Result<u32, Fault> {
        let args = [Val::Bytes(state.to_vec()), Val::U32(from_schema)];
        let result = self.lifecycle("migrate", &args)?;
        Self::start(result, "migrate")
    }

    #[inline(never)]
    fn export(&mut self, to_schema: u32) -> Result<Option<Vec<u8>>, Fault> {
        let bytes = match self.lifecycle("save", &[Val::U32(to_schema)])? {
            Some(Val::Option(None)) => return Ok(None),
            Some(Val::Option(Some(value))) => match *value {
                Val::Bytes(bytes) => bytes,
                Val::List(items) => items
                    .into_iter()
                    .map(|v| match v {
                        Val::U8(b) => Ok(b),
                        _ => Err(Fault::new(
                            Reason::Abi,
                            "save returned a list that is not bytes",
                        )),
                    })
                    .collect::<Result<Vec<u8>, Fault>>()?,
                other => return Err(Fault::new(Reason::Abi, format!("save returned {other:?}"))),
            },
            other => return Err(Fault::new(Reason::Abi, format!("save returned {other:?}"))),
        };
        if bytes.len() > MAX_CHECKPOINT {
            return Err(Fault::new(
                Reason::Abi,
                format!(
                    "a state of {} bytes is over the {MAX_CHECKPOINT}-byte limit",
                    bytes.len()
                ),
            ));
        }
        Ok(Some(bytes))
    }

    fn handle(&mut self, _request: &[u8]) -> Result<Vec<u8>, Fault> {
        Err(Fault::new(
            Reason::Abi,
            "a lifecycle 2 version is called with kai-call 1, not raw requests",
        ))
    }

    #[inline(never)]
    fn health(&mut self) -> Result<u32, Fault> {
        self.code("health")
    }

    #[inline(never)]
    fn self_test(&mut self) -> Result<u32, Fault> {
        self.code("self-test")
    }

    fn quiet(&mut self, quiet: bool) {
        self.store.data_mut().quiet = quiet;
    }

    #[inline(never)]
    fn call(&mut self, func: &appabi::Func, args: &[Val]) -> Result<Option<Val>, Fault> {
        self.typed(func, args, self.limits.fuel_call)
    }
}
