//! A module on the canonical ABI (ADR 0003 section 2), reached through `appabi`:
//! its linear memory, its `cabi_realloc`, and calls to its core exports, each with
//! fresh fuel. Imports are refused for now; `lifecycle 2` linking adds them.

use crate::{Limits, config};
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Trap};
use livecore::{Fault, Reason};
use wasmi::{
    Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, Val,
};

/// An instance of a canonical-ABI module.
pub struct CoreInstance {
    store: Store<StoreLimits>,
    instance: wasmi::Instance,
    memory: Memory,
    realloc: Option<TypedFunc<(i32, i32, i32, i32), i32>>,
    fuel_call: u64,
}

fn trap(error: impl ToString) -> Trap {
    Trap(error.to_string())
}

/// Compiles and instantiates a module with no imports.
#[inline(never)]
pub fn load_core(wasm: &[u8], limits: Limits) -> Result<CoreInstance, Fault> {
    if wasm.len() > kabi::live::MAX_ARTIFACT {
        return Err(Fault::new(Reason::Compile, "the module is too large"));
    }
    let engine = Engine::new(&config());
    let module = Module::new(&engine, wasm)
        .map_err(|error| Fault::new(Reason::Compile, error.to_string()))?;
    if let Some(import) = module.imports().next() {
        let detail = format!(
            "imports {}.{}, and this host links none yet",
            import.module(),
            import.name()
        );
        return Err(Fault::new(Reason::Link, detail));
    }
    let store_limits = StoreLimitsBuilder::new()
        .memory_size(limits.memory_bytes)
        .memories(1)
        .tables(1)
        .instances(1)
        .build();
    let mut store = Store::new(&engine, store_limits);
    store.limiter(|limits| limits);
    store
        .set_fuel(limits.fuel_life)
        .map_err(|error| Fault::new(Reason::Fuel, error.to_string()))?;
    let instance = Linker::<StoreLimits>::new(&engine)
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| Fault::new(Reason::Link, error.to_string()))?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| Fault::new(Reason::Abi, "no memory export"))?;
    let realloc = instance.get_typed_func(&store, "cabi_realloc").ok();
    Ok(CoreInstance {
        store,
        instance,
        memory,
        realloc,
        fuel_call: limits.fuel_call,
    })
}

impl CoreInstance {
    /// Fuel left after the last call.
    pub fn fuel_left(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }
}

impl Guest for CoreInstance {
    #[inline(never)]
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let data = self.memory.data(&self.store);
        let start = at as usize;
        let end = start
            .checked_add(len as usize)
            .ok_or_else(|| trap("read past memory"))?;
        data.get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| trap("read past memory"))
    }

    #[inline(never)]
    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        self.memory
            .write(&mut self.store, at as usize, bytes)
            .map_err(trap)
    }

    #[inline(never)]
    fn realloc(&mut self, old: u32, old_size: u32, align: u32, size: u32) -> Result<u32, Trap> {
        let realloc = self.realloc.ok_or_else(|| trap("no cabi_realloc export"))?;
        self.store.set_fuel(self.fuel_call).map_err(trap)?;
        let args = (old as i32, old_size as i32, align as i32, size as i32);
        realloc
            .call(&mut self.store, args)
            .map(|at| at as u32)
            .map_err(trap)
    }

    #[inline(never)]
    fn call(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap> {
        let func = self
            .instance
            .get_func(&self.store, export)
            .ok_or_else(|| trap(format!("no export {export}")))?;
        let inputs: Vec<Val> = args
            .iter()
            .map(|arg| match *arg {
                Core::I32(v) => Val::I32(v),
                Core::I64(v) => Val::I64(v),
                Core::F32(v) => Val::F32(v.into()),
                Core::F64(v) => Val::F64(v.into()),
            })
            .collect();
        let mut outputs = vec![Val::I32(0); func.ty(&self.store).results().len()];
        self.store.set_fuel(self.fuel_call).map_err(trap)?;
        func.call(&mut self.store, &inputs, &mut outputs)
            .map_err(trap)?;
        outputs
            .into_iter()
            .map(|value| match value {
                Val::I32(v) => Ok(Core::I32(v)),
                Val::I64(v) => Ok(Core::I64(v)),
                Val::F32(v) => Ok(Core::F32(v.into())),
                Val::F64(v) => Ok(Core::F64(v.into())),
                _ => Err(trap("a result outside the core types")),
            })
            .collect()
    }

    fn rep(&mut self, _h: u32) -> Result<u32, Trap> {
        Err(trap("this host has no resources yet"))
    }
}
