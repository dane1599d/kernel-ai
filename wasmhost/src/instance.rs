//! One instance of a version, and the `kai-lifecycle-1` calls on it.

use crate::{Limits, LogSink, MAX_LOG_BYTES, MAX_LOG_LINES};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::MAX_CHECKPOINT;
use livecore::{Fault, Reason};

use wasmi::errors::ErrorKind;
use wasmi::{
    Caller, Engine, Error, Extern, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TrapCode, TypedFunc, WasmParams, WasmResults,
};

/// What host functions see of the instance.
pub(crate) struct HostState {
    pub(crate) limits: StoreLimits,
    /// The instance's own memory region (lifecycle 2), committed as it grows.
    pub(crate) memory: Option<crate::linear::Region>,
    pub(crate) log: Option<LogSink>,
    /// Log lines are dropped while set.
    pub(crate) quiet: bool,
    /// Log lines written during the current call.
    pub(crate) lines: u32,
    /// Where the current call panicked, from `kai.panic`; kept while quiet, so a
    /// rehearsal reports it too.
    pub(crate) panic_at: Option<String>,
    /// `kai:app/clock`'s time and seed, fixed for the call (lifecycle 2).
    pub(crate) now_ms: u64,
    pub(crate) seed: u64,
    /// The action call in progress, with its storage and what it did.
    pub(crate) call: Option<crate::effects::OpenCall>,
    /// The storage lifecycle calls read (`livecore::Instance::view`).
    pub(crate) view: Option<livecore::txn::Storage>,
    /// The app's windows (`kai:ui`, lifecycle 2).
    pub(crate) ui: crate::ui::UiHost,
    /// True only while a real key or click is being handled
    /// ([`crate::CoreInstance::ui_input`], [`crate::CoreInstance::ui_events_user`]).
    /// A host function that must tell a person's input from the agent's
    /// `ui_act` reads it; everything else, replies, events and timers included,
    /// leaves it false, so the default is to refuse. (`kai:edit` carries a
    /// person's mark through the answers to its own messages itself,
    /// [`crate::edit::MAX_CHAIN`], without raising this.)
    pub(crate) user_input: bool,
    /// The settings requests the app sent and comphost has not taken (`kai:settings`).
    pub(crate) settings: crate::settings::SettingsHost,
    /// The edit messages the app sent and comphost has not taken (`kai:edit`).
    pub(crate) edit: crate::edit::EditHost,
}

/// Most bytes of a panic's file path read from the version.
const MAX_PANIC_PATH: usize = 256;

/// A running version: its store (which owns the engine and compiled code) and the
/// exports the contract names.
pub struct WasmInstance {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    schema: TypedFunc<(), i32>,
    init: TypedFunc<(), i32>,
    import: TypedFunc<(i32, i32, i32), i32>,
    export: TypedFunc<i32, i64>,
    handle: TypedFunc<(i32, i32), i64>,
    health: TypedFunc<(), i32>,
    self_test: TypedFunc<(), i32>,
    limits: Limits,
}

fn abi(detail: impl Into<String>) -> Fault {
    Fault::new(Reason::Abi, detail)
}

/// What a failed call into a version means.
pub(crate) fn classify(error: &Error) -> Fault {
    let reason = match (error.as_trap_code(), error.kind()) {
        (Some(TrapCode::OutOfFuel), _) => Reason::Fuel,
        (Some(TrapCode::GrowthOperationLimited | TrapCode::OutOfSystemMemory), _) => Reason::Memory,
        (Some(_), _) => Reason::Trap,
        (None, ErrorKind::Message(_) | ErrorKind::Host(_)) => Reason::Host,
        (None, ErrorKind::Linker(_)) => Reason::Link,
        _ => Reason::Trap,
    };
    Fault::new(reason, error.to_string())
}

/// `kai.log(ptr, len)`: one line from the version, cut to [`MAX_LOG_BYTES`].
pub(crate) fn log(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<(), Error> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| Error::new("log: no memory"))?;
    let (start, len) = (ptr as u32 as usize, len as u32 as usize);
    let bytes = memory
        .data(&caller)
        .get(start..start.saturating_add(len))
        .ok_or_else(|| Error::new("log: text outside memory"))?;
    let text = String::from_utf8_lossy(&bytes[..len.min(MAX_LOG_BYTES)]).into_owned();
    let state = caller.data_mut();
    if let Some(sink) = state.log.filter(|_| !state.quiet)
        && state.lines < MAX_LOG_LINES
    {
        state.lines += 1;
        sink(&text);
    }
    Ok(())
}

/// `kai.panic(file_ptr, file_len, line, column)`: where the version is about to trap.
/// Of a long path only the end is read, which holds the file name a long path is
/// cut to. A path outside memory is ignored; the trap that follows is reported
/// either way.
pub(crate) fn panic(
    mut caller: Caller<'_, HostState>,
    ptr: i32,
    len: i32,
    line: i32,
    column: i32,
) -> Result<(), Error> {
    let memory = caller.get_export("memory").and_then(Extern::into_memory);
    let (start, len) = (ptr as u32 as usize, len as u32 as usize);
    let end = start.saturating_add(len);
    let from = end.saturating_sub(len.min(MAX_PANIC_PATH));
    let path = memory.and_then(|memory| {
        let bytes = memory.data(&caller).get(from..end)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    });
    caller.data_mut().panic_at =
        path.map(|path| livecore::location::panicked(&path, line as u32, column as u32));
    Ok(())
}

/// A failed call's fault; a trap the version announced with `kai.panic` names
/// where it panicked.
pub(crate) fn fault(error: &Error, panic_at: Option<String>) -> Fault {
    match (error.as_trap_code(), panic_at) {
        (Some(TrapCode::UnreachableCodeReached), Some(at)) => Fault::new(Reason::Trap, at),
        _ => classify(error),
    }
}

fn export<P: WasmParams, R: WasmResults>(
    instance: &wasmi::Instance,
    store: &Store<HostState>,
    name: &str,
) -> Result<TypedFunc<P, R>, Fault> {
    instance
        .get_typed_func::<P, R>(store, name)
        .map_err(|_| abi(format!("{name} is missing or has the wrong type")))
}

impl WasmInstance {
    #[inline(never)]
    pub(crate) fn new(
        engine: &Engine,
        module: &Module,
        limits: Limits,
        log_sink: Option<LogSink>,
    ) -> Result<Self, Fault> {
        let state = HostState {
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory_bytes)
                .memories(1)
                .tables(1)
                .table_elements(4096)
                .instances(1)
                .trap_on_grow_failure(true)
                .build(),
            memory: None,
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
            edit: Default::default(),
        };
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(limits.fuel_life)
            .map_err(|error| Fault::new(Reason::Host, error.to_string()))?;
        let mut linker = Linker::<HostState>::new(engine);
        linker
            .func_wrap("kai", "log", log)
            .and_then(|linker| linker.func_wrap("kai", "panic", panic))
            .map_err(|error| Fault::new(Reason::Host, error.to_string()))?;
        let instance =
            linker
                .instantiate_and_start(&mut store, module)
                .map_err(|error| match error.kind() {
                    ErrorKind::Linker(_) | ErrorKind::Instantiation(_) => {
                        Fault::new(Reason::Link, error.to_string())
                    }
                    _ => classify(&error),
                })?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| abi("memory is not exported"))?;
        Ok(Self {
            alloc: export(&instance, &store, "kai_alloc")?,
            schema: export(&instance, &store, "kai_schema")?,
            init: export(&instance, &store, "kai_init")?,
            import: export(&instance, &store, "kai_import")?,
            export: export(&instance, &store, "kai_export")?,
            handle: export(&instance, &store, "kai_handle")?,
            health: export(&instance, &store, "kai_health")?,
            self_test: export(&instance, &store, "kai_self_test")?,
            memory,
            store,
            limits,
        })
    }

    fn call<P: WasmParams, R: WasmResults>(
        &mut self,
        func: TypedFunc<P, R>,
        params: P,
        fuel: u64,
    ) -> Result<R, Fault> {
        self.store
            .set_fuel(fuel)
            .map_err(|error| Fault::new(Reason::Host, error.to_string()))?;
        let state = self.store.data_mut();
        state.lines = 0;
        state.panic_at = None;
        func.call(&mut self.store, params)
            .map_err(|error| fault(&error, self.store.data_mut().panic_at.take()))
    }

    /// Copies `bytes` into memory the version allocated; (ptr, len).
    fn write_input(&mut self, bytes: &[u8]) -> Result<(i32, i32), Fault> {
        let len = i32::try_from(bytes.len()).map_err(|_| abi("input too large"))?;
        let ptr = self.call(self.alloc, len, self.limits.fuel_life)?;
        if ptr == 0 {
            return Err(Fault::new(
                Reason::Memory,
                "kai_alloc found no memory for an input",
            ));
        }
        if !bytes.is_empty() {
            self.memory
                .write(&mut self.store, ptr as u32 as usize, bytes)
                .map_err(|_| abi("kai_alloc returned memory outside the instance"))?;
        }
        Ok((ptr, len))
    }

    /// The bytes a packed `(ptr << 32) | len` result names, at most `max` of them.
    fn read_packed(&self, packed: i64, max: usize) -> Result<Vec<u8>, Fault> {
        let packed = packed as u64;
        let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        if len > max {
            return Err(abi(format!(
                "a result of {len} bytes is over the {max}-byte limit"
            )));
        }
        self.memory
            .data(&self.store)
            .get(ptr..ptr + len)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| abi("a result lies outside memory"))
    }

    fn code(&mut self, func: TypedFunc<(), i32>) -> Result<u32, Fault> {
        self.call(func, (), self.limits.fuel_life)
            .map(|code| code as u32)
    }
}

impl livecore::Instance for WasmInstance {
    #[inline(never)]
    fn schema(&mut self) -> Result<u32, Fault> {
        match self.code(self.schema)? {
            0 => Err(abi("kai_schema returned 0")),
            schema => Ok(schema),
        }
    }

    #[inline(never)]
    fn init(&mut self) -> Result<u32, Fault> {
        self.code(self.init)
    }

    #[inline(never)]
    fn import(&mut self, state: &[u8], from_schema: u32) -> Result<u32, Fault> {
        let (ptr, len) = self.write_input(state)?;
        self.call(
            self.import,
            (ptr, len, from_schema as i32),
            self.limits.fuel_life,
        )
        .map(|code| code as u32)
    }

    #[inline(never)]
    fn export(&mut self, to_schema: u32) -> Result<Option<Vec<u8>>, Fault> {
        let to = i32::try_from(to_schema).map_err(|_| abi("schema out of range"))?;
        match self.call(self.export, to, self.limits.fuel_life)? {
            -1 => Ok(None),
            packed => self.read_packed(packed, MAX_CHECKPOINT).map(Some),
        }
    }

    #[inline(never)]
    fn handle(&mut self, request: &[u8]) -> Result<Vec<u8>, Fault> {
        let (ptr, len) = self.write_input(request)?;
        let packed = self.call(self.handle, (ptr, len), self.limits.fuel_call)?;
        self.read_packed(packed, kabi::MAX_MESSAGE)
    }

    #[inline(never)]
    fn health(&mut self) -> Result<u32, Fault> {
        self.code(self.health)
    }

    #[inline(never)]
    fn self_test(&mut self) -> Result<u32, Fault> {
        self.code(self.self_test)
    }

    #[inline(never)]
    fn quiet(&mut self, quiet: bool) {
        self.store.data_mut().quiet = quiet;
    }
}

impl wasmi::ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmi_core::LimiterError> {
        let allowed = self.limits.memory_growing(current, desired, maximum)?;
        // Commit before wasmi writes the new pages.
        // The system out of memory fails the guest's `memory.grow` (-1); the
        // guest decides what that means.
        Ok(match self.memory.as_mut() {
            Some(region) if allowed => region.grow(desired),
            _ => allowed,
        })
    }

    fn memory_grow_failed(
        &mut self,
        error: &wasmi::errors::MemoryError,
    ) -> Result<(), wasmi_core::LimiterError> {
        self.limits.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmi_core::LimiterError> {
        self.limits.table_growing(current, desired, maximum)
    }

    fn table_grow_failed(
        &mut self,
        error: &wasmi::errors::TableError,
    ) -> Result<(), wasmi_core::LimiterError> {
        self.limits.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.limits.instances()
    }

    fn tables(&self) -> usize {
        self.limits.tables()
    }

    fn memories(&self) -> usize {
        self.limits.memories()
    }
}
