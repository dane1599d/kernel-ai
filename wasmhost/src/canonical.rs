//! A `lifecycle 2` version (ADR 0003 sections 2-3): a module on the canonical ABI,
//! reached through `appabi`. Its `kai:*` imports are linked by canonical version
//! from the generated tables: `kai:app/log` and `kai:app/clock`, `storage` and
//! `effects` with the effect transaction, and `kai:ui/view` (`ui`). Its
//! `kai:live/lifecycle` exports serve `livecore::Instance`; app exports are
//! reached with `Instance::call`. Each call gets fresh fuel, and time and the seed
//! are fixed for the call.

use crate::effects::{self, OpenCall};
use crate::grants::Grants;
use crate::instance::{HostState, classify, fault, log, panic};
use crate::link::{self, WitFunc};
use crate::{Limits, LogSink, config, tables};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Trap, Val};
use kabi::live::MAX_CHECKPOINT;
use livecore::actions::Actions;
use livecore::call::{self, CallError};
use livecore::{Fault, Reason};
use wasmi::errors::ErrorKind;
use wasmi::{
    Caller, Engine, Error, ExternType, Linker, Memory, Module, Store, StoreLimitsBuilder, TypedFunc,
};

/// An instance of a canonical-ABI module.
pub struct CoreInstance {
    pub(crate) store: Store<HostState>,
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
    /// The app's action rows, which type its `kai-call 1` requests.
    actions: Option<Actions>,
    /// The `kai:ui/input` `on-ui` export, when the module has one.
    pub(crate) on_ui: Option<String>,
    /// The `kai:code/code-input` exports the module has.
    pub(crate) code: alloc::boxed::Box<crate::code::CodeExports>,
    /// Whether the last guest call was made while a person's key or click was
    /// handled ([`CoreInstance::last_call_by_user`]).
    pub(crate) by_user: bool,
    /// The export `kai:app/events.on-event` is, if the app has one.
    pub(crate) on_event: Option<String>,
    /// The `kai:settings/replies` `on-reply` export, when the module has one.
    pub(crate) on_settings_reply: Option<String>,
}

impl CoreInstance {
    /// Gives the instance the action rows its requests are typed by.
    pub fn set_actions(&mut self, actions: Actions) {
        self.actions = Some(actions);
    }

    /// Whether the most recent guest call was made while a person's key or
    /// click was being handled (`ui_input`, `ui_events_user`), not for the
    /// agent's `ui_act`, an event or anything else.
    pub fn last_call_by_user(&self) -> bool {
        self.by_user
    }
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
    load_core_granted(wasm, limits, log_sink, Grants::default())
}

/// As [`load_core`], for a version whose package holds `grants`: without a
/// grant, an import of the interface it opens does not link.
#[inline(never)]
pub fn load_core_granted(
    wasm: &[u8],
    limits: Limits,
    log_sink: Option<LogSink>,
    grants: Grants,
) -> Result<CoreInstance, Fault> {
    if wasm.len() > kabi::live::MAX_ARTIFACT {
        return Err(Fault::new(Reason::Compile, "the module is too large"));
    }
    let engine = Engine::new(&config());
    // The guest's memory, moved into a region of its own.
    let moved = crate::linear::own_memory_as_import(wasm);
    let bytes = moved.as_ref().map_or(wasm, |(moved, _)| moved.as_slice());
    let module = Module::new(&engine, bytes)
        .map_err(|error| Fault::new(Reason::Compile, error.to_string()))?;
    let mut linker = link_imports(&engine, &module, grants)?;
    let state = HostState {
        limits: StoreLimitsBuilder::new()
            .memory_size(limits.memory_bytes)
            // Its own region's memory, made by the host, counts as one more.
            .memories(2)
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
        call: None,
        view: None,
        ui: Default::default(),
        user_input: false,
        settings: Default::default(),
        memory: None,
    };
    let mut store = Store::new(&engine, state);
    store.limiter(|state| state);
    if let Some((_, pages)) = moved {
        let memory = own_memory(&mut store, pages, limits.memory_bytes)?;
        linker
            .define(crate::linear::MODULE, crate::linear::NAME, memory)
            .map_err(|error| link_err(error.to_string()))?;
    }
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
    let on_ui = tables::EXPORTS
        .iter()
        .find(|f| f.interface == crate::ui::INPUT && f.name == "on-ui")
        .and_then(|f| export_for(&module, f));
    let on_event = tables::EXPORTS
        .iter()
        .find(|f| f.interface == "kai:app/events" && f.name == "on-event")
        .and_then(|f| export_for(&module, f));
    let on_settings_reply = tables::EXPORTS
        .iter()
        .find(|f| f.interface == crate::settings::REPLIES && f.name == "on-reply")
        .and_then(|f| export_for(&module, f));
    Ok(CoreInstance {
        store,
        instance,
        memory,
        realloc,
        limits,
        fuel: limits.fuel_call,
        last: None,
        lifecycle,
        actions: None,
        on_ui,
        code: alloc::boxed::Box::new(crate::code::CodeExports::of(&module)),
        by_user: false,
        on_event,
        on_settings_reply,
    })
}

/// The guest's memory on a region of its own: as large as the cap (or the
/// guest's maximum, if lower) can reach, its initial pages committed.
fn own_memory(
    store: &mut Store<HostState>,
    pages: crate::linear::Limits,
    cap: usize,
) -> Result<Memory, Fault> {
    use crate::linear::{PAGE, Region};
    let most = pages.max.map_or(cap, |max| cap.min(max as usize * PAGE));
    let capacity = most / PAGE * PAGE;
    let initial = pages.min as usize * PAGE;
    let too_big = || Fault::new(Reason::Memory, "its memory starts larger than its cap");
    let mut region = Region::new(capacity, initial).ok_or_else(too_big)?;
    // SAFETY: the region moves into the store's data below and is dropped with
    // the store, after the memory that uses the buffer.
    let buffer = unsafe { region.buffer() };
    let ty = wasmi::MemoryType::new(pages.min, pages.max);
    let memory = Memory::new_static(&mut *store, ty, buffer)
        .map_err(|error| Fault::new(Reason::Memory, error.to_string()))?;
    store.data_mut().memory = Some(region);
    Ok(memory)
}

/// Binds every import the module makes, or fails naming the first it cannot.
fn link_imports(
    engine: &Engine,
    module: &Module,
    grants: Grants,
) -> Result<Linker<HostState>, Fault> {
    let mut linker = Linker::<HostState>::new(engine);
    for import in module.imports() {
        let (qualified, name) = (import.module(), import.name());
        // Its own memory, which `load_core` defines.
        if (qualified, name) == (crate::linear::MODULE, crate::linear::NAME) {
            continue;
        }
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
            ("kai:app/storage", "get") => linker.func_wrap(qualified, name, effects::get),
            ("kai:app/storage", "put") => linker.func_wrap(qualified, name, effects::put),
            ("kai:app/storage", "delete") => linker.func_wrap(qualified, name, effects::delete),
            ("kai:app/effects", "request") => linker.func_wrap(qualified, name, effects::request),
            (crate::settings::SETTINGS, _) if !crate::settings::granted(grants) => {
                return Err(link_err(format!(
                    "{qualified} needs the settings grant, which this app's package does not hold"
                )));
            }
            (crate::settings::SETTINGS, "send") => {
                linker.func_wrap(qualified, name, crate::settings::send)
            }
            (crate::code::CODE, _) | (crate::ui::VIEW, _) => linker.func_new(
                qualified,
                name,
                ty.clone(),
                move |caller, params, _results| crate::ui::view(caller, entry, params),
            ),
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
pub(crate) fn export_for(module: &Module, entry: &WitFunc) -> Option<String> {
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
        let by_user = state.user_input;
        self.by_user = by_user;
        self.settings_call_begins();
        let result = appabi::call(self, func, args).map_err(|trap| {
            self.last
                .take()
                .unwrap_or_else(|| Fault::new(Reason::Abi, trap.0))
        });
        if result.is_err() {
            self.settings_call_faulted();
        }
        result
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

    /// A `kai-call 1` request, typed by the action rows. A request the rows do not
    /// allow is refused in the reply; only the version's own failure is a fault.
    fn handle(&mut self, request: &[u8]) -> Result<Vec<u8>, Fault> {
        let Some(actions) = self.actions.take() else {
            return Ok(call::format_refusal("this version declares no actions").into_bytes());
        };
        let reply = match core::str::from_utf8(request) {
            Ok(text) => call::serve(self, &actions, text),
            Err(_) => Err(CallError::Request("the request is not text".into())),
        };
        self.actions = Some(actions);
        match reply {
            Ok(reply) => Ok(reply.into_bytes()),
            Err(CallError::Request(why)) => Ok(call::format_refusal(&why).into_bytes()),
            Err(CallError::Fault(fault)) => Err(fault),
        }
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

    fn begin(&mut self, storage: &livecore::txn::Storage, inputs: livecore::txn::Inputs) {
        self.set_call_inputs(inputs.now_ms, inputs.seed);
        self.store.data_mut().call = Some(OpenCall {
            storage: storage.clone(),
            txn: livecore::txn::Txn::default(),
            first_ticket: inputs.first_ticket,
        });
    }

    fn view(&mut self, storage: &livecore::txn::Storage) {
        self.store.data_mut().view = Some(storage.clone());
    }

    fn finish(&mut self) -> livecore::txn::Txn {
        self.store
            .data_mut()
            .call
            .take()
            .map(|call| call.txn)
            .unwrap_or_default()
    }
}
