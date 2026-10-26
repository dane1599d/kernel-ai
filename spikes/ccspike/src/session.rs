//! The session's Store: one memory, one funcref table and one stack pointer that
//! every linked entry imports, and the spike's `kai` imports:
//!
//! - `kai.print_i64(i64)` and `kai.print_str(ptr, len)` append to the output;
//! - `kai.need(i64) -> i64` is an effect: the host call fails with [`Need`], wasmi
//!   suspends the run (`ResumableCallHostTrap`), and the harness resumes it with
//!   the answer `x + 1`, as `cc` would after the shell's `resume` (ADR 0012 D5).

use crate::{Clock, engine};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
use core::fmt;
use kailink::{FuncType as LinkType, Layout, Linked, ValType as LinkVal};
use wasmi::{
    CompilationMode, Engine, Extern, FuncType, Global, Linker, Memory, MemoryType, Module,
    Mutability, Ref, RefType, ResumableCall, Store, Table, TableType, Val, ValType,
};

const PAGE: u32 = 64 << 10;

/// The error `kai.need` raises to suspend the run.
#[derive(Debug)]
pub struct Need(pub i64);

impl fmt::Display for Need {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "need {}", self.0)
    }
}

impl wasmi::errors::HostError for Need {}

#[derive(Default)]
pub struct Host {
    pub out: String,
}

/// One entry's link, instantiate and run.
#[derive(Debug, Default)]
pub struct Ran {
    pub link_ns: u64,
    pub instantiate_ns: u64,
    pub run_ns: u64,
    pub result: Option<i64>,
    pub fuel: u64,
    pub slices: u32,
    /// Each need/resume round trip, in nanoseconds.
    pub needs: Vec<u64>,
    pub retyped: Vec<String>,
}

pub struct Session {
    engine: Engine,
    store: Store<Host>,
    linker: Linker<Host>,
    memory: Memory,
    table: Table,
    link: kailink::Session,
    pub slice: u64,
}

fn allowed(field: &str, ty: &LinkType) -> bool {
    use LinkVal::{I32, I64};
    let (params, results): (&[LinkVal], &[LinkVal]) = match field {
        "print_i64" => (&[I64], &[]),
        "print_str" => (&[I32, I32], &[]),
        "need" => (&[I64], &[I64]),
        _ => return false,
    };
    ty.params == params && ty.results == results
}

impl Session {
    pub fn new(mode: CompilationMode) -> Result<Self, String> {
        let layout = Layout::default();
        let engine = Engine::new(&engine::config(mode, true));
        let mut store = Store::new(&engine, Host::default());
        let memory = Memory::new(
            &mut store,
            MemoryType::new(16, Some(layout.memory_limit / PAGE)),
        )
        .map_err(|e| e.to_string())?;
        let table = Table::new(
            &mut store,
            TableType::new(RefType::Func, 1, Some(layout.table_limit)),
            Ref::null(RefType::Func),
        )
        .map_err(|e| e.to_string())?;
        let sp = Global::new(
            &mut store,
            Val::I32(layout.data_base as i32),
            Mutability::Var,
        );
        let mut linker = Linker::new(&engine);
        let define = |linker: &mut Linker<Host>, name: &str, item: Extern| {
            linker
                .define(kailink::ENV, name, item)
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        define(&mut linker, kailink::MEMORY, memory.into())?;
        define(&mut linker, kailink::TABLE, table.into())?;
        define(&mut linker, kailink::STACK_POINTER, sp.into())?;
        define_kai(&mut linker, memory)?;
        Ok(Self {
            engine,
            store,
            linker,
            memory,
            table,
            link: kailink::Session::new(layout),
            slice: 10_000_000,
        })
    }

    pub fn output(&self) -> &str {
        &self.store.data().out
    }

    pub fn take_output(&mut self) -> String {
        core::mem::take(&mut self.store.data_mut().out)
    }

    pub fn symbol(&self, name: &str) -> Option<&kailink::Symbol> {
        self.link.symbol(name)
    }

    /// Links, instantiates and runs one object; `run` is the function to call,
    /// if any. Definitions are committed only if all of it succeeds.
    pub fn eval(&mut self, object: &[u8], run: Option<&str>, clock: Clock) -> Result<Ran, String> {
        let mut ran = Ran::default();
        let start = clock();
        let linked = self
            .link
            .link(object, &allowed)
            .map_err(|e| format!("link: {e}"))?;
        ran.link_ns = clock() - start;
        let start = clock();
        let instance = self.instantiate(&linked)?;
        ran.instantiate_ns = clock() - start;
        if let Some(name) = run {
            let func = instance
                .get_func(&self.store, name)
                .ok_or_else(|| format!("no function {name}"))?;
            let start = clock();
            self.run(func, &mut ran, clock)?;
            ran.run_ns = clock() - start;
        }
        ran.retyped = linked.retyped.clone();
        self.link.commit(&linked);
        Ok(ran)
    }

    fn instantiate(&mut self, linked: &Linked) -> Result<wasmi::Instance, String> {
        let slots = u64::from(linked.table_size);
        let have = self.table.size(&self.store);
        if slots > have {
            self.table
                .grow(&mut self.store, slots - have, Ref::null(RefType::Func))
                .map_err(|e| format!("table: {e}"))?;
        }
        let bytes = self.memory.data(&self.store).len() as u64;
        let need = u64::from(linked.data_end);
        if need > bytes {
            let pages = (need - bytes).div_ceil(u64::from(PAGE));
            self.memory
                .grow(&mut self.store, pages)
                .map_err(|e| format!("memory: {e}"))?;
        }
        let module = Module::new(&self.engine, &linked.wasm).map_err(|e| format!("module: {e}"))?;
        self.linker
            .instantiate_and_start(&mut self.store, &module)
            .map_err(|e| format!("instantiate: {e}"))
    }

    fn run(&mut self, func: wasmi::Func, ran: &mut Ran, clock: Clock) -> Result<(), String> {
        let ty = func.ty(&self.store);
        let mut results = vec![Val::I32(0); ty.results().len()];
        self.store.set_fuel(self.slice).map_err(|e| e.to_string())?;
        ran.slices = 1;
        let mut call = func.call_resumable(&mut self.store, &[], &mut results);
        loop {
            match call.map_err(|e| format!("trap: {e}"))? {
                ResumableCall::Finished => break,
                ResumableCall::OutOfFuel(out) => {
                    ran.slices += 1;
                    ran.fuel += self.slice - self.store.get_fuel().unwrap_or(0);
                    self.store.set_fuel(self.slice).map_err(|e| e.to_string())?;
                    call = out.resume(&mut self.store, &mut results);
                }
                ResumableCall::HostTrap(trap) => {
                    let start = clock();
                    let Some(Need(x)) = trap.host_error().downcast_ref::<Need>() else {
                        return Err(format!("host trap: {}", trap.host_error()));
                    };
                    let answer = [Val::I64(x + 1)];
                    call = trap.resume(&mut self.store, &answer, &mut results);
                    ran.needs.push(clock() - start);
                }
            }
        }
        ran.fuel += self.slice - self.store.get_fuel().unwrap_or(0);
        ran.result = results.first().map(|v| match v {
            Val::I32(x) => i64::from(*x),
            Val::I64(x) => *x,
            _ => 0,
        });
        Ok(())
    }
}

fn define_kai(linker: &mut Linker<Host>, memory: Memory) -> Result<(), String> {
    let err = |e: wasmi::errors::LinkerError| e.to_string();
    linker
        .func_wrap(
            kailink::KAI,
            "print_i64",
            |mut caller: wasmi::Caller<Host>, x: i64| {
                let out = &mut caller.data_mut().out;
                out.push_str(&x.to_string());
                out.push('\n');
            },
        )
        .map_err(err)?;
    linker
        .func_wrap(
            kailink::KAI,
            "print_str",
            move |mut caller: wasmi::Caller<Host>,
                  ptr: i32,
                  len: i32|
                  -> Result<(), wasmi::Error> {
                let (mem, host) = memory.data_and_store_mut(&mut caller);
                let bytes = (ptr as u32 as usize)
                    .checked_add(len as u32 as usize)
                    .and_then(|end| mem.get(ptr as u32 as usize..end))
                    .ok_or_else(|| wasmi::Error::new("print_str out of bounds"))?;
                host.out.push_str(&String::from_utf8_lossy(bytes));
                Ok(())
            },
        )
        .map_err(err)?;
    let ty = FuncType::new([ValType::I64], [ValType::I64]);
    linker
        .func_new(kailink::KAI, "need", ty, |_caller, args, _out| {
            let Val::I64(x) = args[0] else {
                return Err(wasmi::Error::new("need takes an i64"));
            };
            Err(wasmi::Error::host(Need(x)))
        })
        .map_err(err)?;
    Ok(())
}
