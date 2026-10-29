//! The session's Store (ADR 0012 D4): one memory (1 MiB, growing to 64 MiB),
//! one funcref table (at most 4,096 slots) and one stack pointer, shared by
//! every module kailink links into it. kairt is linked first.
//!
//! An entry's module is instantiated before its run function is called, and
//! instantiation writes its functions into their table slots. So the slots it
//! writes are saved first: a trap puts them back, and so does `undo` later. Its
//! data and slots stay reserved either way, since a stored pointer may reach
//! them.
//!
//! Session code reaches only its imports: kairt's `fd_write` for output, the
//! `__show_*` calls of kai.h, and the effects the service allows. An effect
//! never runs here: its import fails with [`Need`], wasmi suspends the run
//! (`ResumableCallHostTrap`), and the run goes on only when the shell's answer
//! comes back through [`Session::resume`] (D5).

mod imports;

use crate::{Budget, Host, LINE_NS, Stopped, dis, engine};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use imports::{Need, State, allowed, define_builtins, define_effect, reason};
use kailink::{FuncType as LinkType, Layout, Linked, Symbol};
use promptcore::wire::ValueType;
use wasmi::{
    Engine, Error, Extern, Global, Linker, Memory, MemoryType, Module, Mutability, Ref, RefType,
    ResumableCall, ResumableCallHostTrap, Store, Table, TableType, Val,
};

const PAGE: u32 = 64 << 10;
/// Output one entry may print (D3, step 7).
pub const MAX_OUTPUT: usize = 6 << 10;
/// Longest string a shown value reads.
pub const MAX_SHOWN: usize = 1024;
/// Most modules one Store holds. wasmi keeps every instance until the Store
/// drops; D4's compaction would relink them, and until it is built `:reset`
/// starts again.
pub const MAX_ENTRIES: usize = 512;
/// Fuel per slice of a run.
pub const SLICE: u64 = 1_000_000;

/// An entry that ran.
#[derive(Debug)]
pub struct Done {
    pub defs: Vec<kailink::Def>,
    pub retyped: Vec<String>,
    pub value: Option<(ValueType, Vec<u8>)>,
    pub link_ns: u64,
    pub run_ns: u64,
}

#[derive(Debug)]
pub enum Step {
    Done(Done),
    /// Suspended on effect `name`, called with `args` (each as its bits).
    Need {
        name: String,
        args: Vec<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Link(kailink::Error),
    /// A trap, by its reason word.
    Trap(&'static str),
    Stopped(Stopped),
    /// [`MAX_ENTRIES`] modules.
    Full,
    Internal(String),
}

/// An entry being run: what a failure or a commit needs.
struct Running {
    seq: u64,
    linked: Linked,
    slots: Vec<(u32, Ref)>,
    link_ns: u64,
    run_ns: u64,
    /// The stack pointer before the run: a trap leaves it wherever the
    /// trapping function had moved it.
    sp: Val,
}

struct Pending {
    running: Running,
    call: ResumableCallHostTrap,
}

/// A committed entry: what `undo` puts back, and its module for `:dis`.
struct Kept {
    seq: u64,
    commit: kailink::Commit,
    slots: Vec<(u32, Ref)>,
    wasm: Vec<u8>,
    exports: Vec<String>,
}

pub struct Session {
    engine: Engine,
    store: Store<State>,
    linker: Linker<State>,
    memory: Memory,
    table: Table,
    sp: Global,
    link: kailink::Session,
    effects: Vec<(String, LinkType)>,
    /// kairt's module, for `:dis`.
    base: (Vec<u8>, Vec<String>),
    kept: Vec<Kept>,
    pending: Option<Box<Pending>>,
    instances: usize,
}

impl Session {
    /// A Store with kairt's object linked in; `effects` are the `kai` imports
    /// that suspend the run.
    pub fn new(kairt: &[u8], effects: Vec<(String, LinkType)>) -> Result<Self, String> {
        let layout = Layout::default();
        let engine = Engine::new(&engine::config(true));
        let mut store = Store::new(&engine, State::default());
        let memory_type = MemoryType::new(16, Some(layout.memory_limit / PAGE));
        let memory = Memory::new(&mut store, memory_type).map_err(|e| e.to_string())?;
        let table_type = TableType::new(RefType::Func, 1, Some(layout.table_limit));
        let table = Table::new(&mut store, table_type, Ref::null(RefType::Func))
            .map_err(|e| e.to_string())?;
        let sp = Global::new(
            &mut store,
            Val::I32(layout.data_base as i32),
            Mutability::Var,
        );
        let mut linker = Linker::new(&engine);
        for (name, item) in [
            (kailink::MEMORY, Extern::from(memory)),
            (kailink::TABLE, table.into()),
            (kailink::STACK_POINTER, sp.into()),
        ] {
            linker
                .define(kailink::ENV, name, item)
                .map_err(|e| e.to_string())?;
        }
        define_builtins(&mut linker, memory).map_err(|e| e.to_string())?;
        for (name, ty) in &effects {
            define_effect(&mut linker, name, ty).map_err(|e| e.to_string())?;
        }
        let mut session = Self {
            engine,
            store,
            linker,
            memory,
            table,
            sp,
            link: kailink::Session::new(layout),
            effects,
            base: (Vec::new(), Vec::new()),
            kept: Vec::new(),
            pending: None,
            instances: 0,
        };
        let linked = session
            .link
            .link(kairt, &|field, ty| allowed(&[], field, ty))
            .map_err(|e| format!("kairt: {e}"))?;
        session.grow(&linked).map_err(|e| format!("kairt: {e}"))?;
        session
            .instantiate(&linked)
            .map_err(|e| format!("kairt: {e}"))?;
        let _ = session.link.commit(&linked);
        session.base = (linked.wasm, linked.exports);
        Ok(session)
    }

    /// Links, instantiates and runs entry `seq`'s object; `run` is its run
    /// function, given the 2 s line budget. Drops a suspended run first.
    pub fn start(
        &mut self,
        seq: u64,
        object: &[u8],
        run: Option<&str>,
        host: &dyn Host,
    ) -> Result<Step, Failure> {
        self.abort();
        let budget = &Budget::new(host, LINE_NS);
        if self.instances >= MAX_ENTRIES {
            return Err(Failure::Full);
        }
        let start = budget.now();
        let effects = &self.effects;
        let allow = |field: &str, ty: &LinkType| allowed(effects, field, ty);
        let linked = self.link.link(object, &allow).map_err(Failure::Link)?;
        self.grow(&linked).map_err(Failure::Internal)?;
        let slots = self.save_slots(&linked);
        let running = Running {
            seq,
            linked,
            slots,
            link_ns: budget.now().saturating_sub(start),
            run_ns: 0,
            sp: self.sp.get(&self.store),
        };
        let instance = match self.instantiate(&running.linked) {
            Ok(instance) => instance,
            Err(e) => {
                self.fail(&running);
                return Err(Failure::Internal(e));
            }
        };
        let Some(name) = run else {
            return Ok(Step::Done(self.finish(running)));
        };
        let Some(func) = instance.get_func(&self.store, name) else {
            self.fail(&running);
            return Err(Failure::Internal(format!("no function {name}")));
        };
        self.set_fuel(&running)?;
        let started = budget.now();
        let call = func.call_resumable(&mut self.store, &[], &mut []);
        self.drive(running, call, started, budget)
    }

    /// Goes on with a suspended run; `value` is the effect's result. The time
    /// spent waiting does not count against the run's budget.
    pub fn resume(&mut self, value: i64, host: &dyn Host) -> Result<Step, Failure> {
        let Some(pending) = self.pending.take() else {
            return Err(Failure::Internal("no run is waiting".into()));
        };
        let Pending { running, call } = *pending;
        let budget = &Budget::new(host, LINE_NS.saturating_sub(running.run_ns));
        // Effects answered at once come back here without the run running out
        // of fuel, so the budget is checked on every resume too.
        if let Err(stop) = budget.check() {
            self.fail(&running);
            return Err(Failure::Stopped(stop));
        }
        self.set_fuel(&running)?;
        let started = budget.now();
        let call = call.resume(&mut self.store, &[Val::I64(value)], &mut []);
        self.drive(running, call, started, budget)
    }

    /// The effect a suspended run waits on.
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Drops a suspended run as a failure: nothing it defined stays.
    pub fn abort(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.fail(&pending.running);
        }
    }

    /// Reverts the last committed entry: its symbols, and the slots it wrote.
    /// Returns its seq.
    pub fn undo(&mut self) -> Option<u64> {
        self.abort();
        let kept = self.kept.pop()?;
        self.link.revert(kept.commit);
        self.restore(&kept.slots);
        Some(kept.seq)
    }

    /// `:dis name`: the Wasm text of the live function `name`.
    pub fn dis(&self, name: &str) -> Option<String> {
        if !matches!(self.link.symbol(name), Some(Symbol::Func { .. })) {
            return None;
        }
        let slot_name = |slot: u32| {
            self.link.symbols().find_map(|(n, s)| match s {
                Symbol::Func { slot: at, .. } if *at == slot => Some(n.to_string()),
                _ => None,
            })
        };
        let kept = self.kept.iter().rev().map(|k| (&k.wasm, &k.exports));
        let base = core::iter::once((&self.base.0, &self.base.1));
        kept.chain(base)
            .find(|(_, exports)| exports.iter().any(|e| e == name))
            .and_then(|(wasm, _)| dis::function(wasm, name, &slot_name))
    }

    /// The output printed since the last call, and whether some was cut.
    pub fn take_output(&mut self) -> (Vec<u8>, bool) {
        let state = self.store.data_mut();
        (
            core::mem::take(&mut state.out),
            core::mem::take(&mut state.cut),
        )
    }

    pub fn memory(&self) -> &[u8] {
        self.memory.data(&self.store)
    }

    pub fn memory_mut(&mut self) -> &mut [u8] {
        self.memory.data_mut(&mut self.store)
    }

    fn set_fuel(&mut self, running: &Running) -> Result<(), Failure> {
        match self.store.set_fuel(SLICE) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.fail(running);
                Err(Failure::Internal(e.to_string()))
            }
        }
    }

    fn drive(
        &mut self,
        mut running: Running,
        mut call: Result<ResumableCall, Error>,
        started: u64,
        budget: &Budget,
    ) -> Result<Step, Failure> {
        loop {
            match call {
                Ok(ResumableCall::Finished) => {
                    running.run_ns += budget.now().saturating_sub(started);
                    return Ok(Step::Done(self.finish(running)));
                }
                Ok(ResumableCall::OutOfFuel(out)) => {
                    if let Err(stop) = budget.check() {
                        self.fail(&running);
                        return Err(Failure::Stopped(stop));
                    }
                    self.set_fuel(&running)?;
                    call = out.resume(&mut self.store, &mut []);
                }
                Ok(ResumableCall::HostTrap(trap)) => {
                    running.run_ns += budget.now().saturating_sub(started);
                    let Some(need) = trap.host_error().downcast_ref::<Need>() else {
                        self.fail(&running);
                        return Err(Failure::Trap("host"));
                    };
                    let step = Step::Need {
                        name: need.name.clone(),
                        args: need.args.clone(),
                    };
                    self.pending = Some(Box::new(Pending {
                        running,
                        call: trap,
                    }));
                    return Ok(step);
                }
                Err(error) => {
                    self.fail(&running);
                    return Err(Failure::Trap(reason(&error)));
                }
            }
        }
    }

    fn finish(&mut self, running: Running) -> Done {
        let Running {
            seq,
            linked,
            slots,
            link_ns,
            run_ns,
            sp: _,
        } = running;
        let commit = self.link.commit(&linked);
        self.kept.push(Kept {
            seq,
            commit,
            slots,
            wasm: linked.wasm,
            exports: linked.exports,
        });
        Done {
            defs: linked.defs,
            retyped: linked.retyped,
            value: self.store.data_mut().value.take(),
            link_ns,
            run_ns,
        }
    }

    fn fail(&mut self, running: &Running) {
        self.restore(&running.slots);
        self.link.reserve(&running.linked);
        self.store.data_mut().value = None;
        // The global is the session's own mutable i32, so setting it back to
        // its earlier value cannot fail.
        let _ = self.sp.set(&mut self.store, running.sp.clone());
    }

    /// Grows the table and memory to hold `linked`.
    fn grow(&mut self, linked: &Linked) -> Result<(), String> {
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
        Ok(())
    }

    /// The slots `linked` writes when instantiated, with what they hold now:
    /// its functions' slots, and every slot it adds.
    fn save_slots(&self, linked: &Linked) -> Vec<(u32, Ref)> {
        let mut slots: Vec<u32> = linked
            .defs
            .iter()
            .filter_map(|d| match d.symbol {
                Symbol::Func { slot, .. } => Some(slot),
                Symbol::Data { .. } => None,
            })
            .chain(self.link.table_size()..linked.table_size)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
            .into_iter()
            .map(|slot| {
                let old = self.table.get(&self.store, u64::from(slot));
                (slot, old.unwrap_or(Ref::null(RefType::Func)))
            })
            .collect()
    }

    fn restore(&mut self, slots: &[(u32, Ref)]) {
        for (slot, old) in slots {
            // The table only grows, so every saved slot is still in it.
            let _ = self.table.set(&mut self.store, u64::from(*slot), *old);
        }
    }

    fn instantiate(&mut self, linked: &Linked) -> Result<wasmi::Instance, String> {
        let module = Module::new(&self.engine, &linked.wasm).map_err(|e| format!("module: {e}"))?;
        let instance = self
            .linker
            .instantiate_and_start(&mut self.store, &module)
            .map_err(|e| format!("instantiate: {e}"))?;
        self.instances += 1;
        Ok(instance)
    }
}
