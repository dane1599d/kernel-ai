//! Spike S03-1's harness: the probe's types from WIT, the guest on wasmi through
//! spike `appabi`, the same bytes wrapped by wit-component on Wasmtime 50, and a
//! generator of calls. The measures are the tests in `tests/`.

use appabi_spike::{Core, Func, Guest, Trap, Ty, Val};
use std::path::{Path, PathBuf};
use std::process::Command;
use wit_parser::{Handle, Resolve, Type, TypeDefKind};

pub const INTERFACE: &str = "kai:probe/api@0.1.0";

/// The spike directory.
pub fn spike_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Builds a guest crate under the spike for wasm32 and returns its module bytes.
pub fn build_guest(crate_dir: &str, artifact: &str) -> Vec<u8> {
    let dir = spike_dir().join(crate_dir);
    let target = spike_dir().join("../../target/s03-1-guests");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building {crate_dir} failed");
    let path = target.join(format!("wasm32-unknown-unknown/release/{artifact}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every function of the probe's `api` interface, typed for `appabi`, by WIT name.
pub fn probe_funcs() -> Vec<(String, Func)> {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_file(spike_dir().join("wit/probe.wit"))
        .expect("probe.wit resolves");
    let api = resolve.packages[package].interfaces["api"];
    let iface = &resolve.interfaces[api];
    iface
        .functions
        .iter()
        .map(|(name, f)| {
            let func = Func {
                export: format!("{INTERFACE}#{name}"),
                params: f.params.iter().map(|p| ty(&resolve, &p.ty)).collect(),
                result: f.result.as_ref().map(|t| ty(&resolve, t)),
            };
            (name.clone(), func)
        })
        .collect()
}

fn ty(resolve: &Resolve, t: &Type) -> Ty {
    match t {
        Type::Bool => Ty::Bool,
        Type::U8 => Ty::U8,
        Type::U16 => Ty::U16,
        Type::U32 => Ty::U32,
        Type::U64 => Ty::U64,
        Type::S8 => Ty::S8,
        Type::S16 => Ty::S16,
        Type::S32 => Ty::S32,
        Type::S64 => Ty::S64,
        Type::F32 => Ty::F32,
        Type::F64 => Ty::F64,
        Type::Char => Ty::Char,
        Type::String => Ty::String,
        Type::ErrorContext => panic!("error-context is outside the subset"),
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Record(r) => Ty::Record(
                r.fields
                    .iter()
                    .map(|f| (f.name.clone(), ty(resolve, &f.ty)))
                    .collect(),
            ),
            TypeDefKind::Tuple(t) => Ty::Tuple(t.types.iter().map(|t| ty(resolve, t)).collect()),
            TypeDefKind::Variant(v) => Ty::Variant(
                v.cases
                    .iter()
                    .map(|c| (c.name.clone(), c.ty.as_ref().map(|t| ty(resolve, t))))
                    .collect(),
            ),
            TypeDefKind::Enum(e) => Ty::Enum(e.cases.iter().map(|c| c.name.clone()).collect()),
            TypeDefKind::Flags(f) => Ty::Flags(f.flags.iter().map(|f| f.name.clone()).collect()),
            TypeDefKind::Option(t) => Ty::Option(Box::new(ty(resolve, t))),
            TypeDefKind::Result(r) => Ty::Result(
                r.ok.as_ref().map(|t| Box::new(ty(resolve, t))),
                r.err.as_ref().map(|t| Box::new(ty(resolve, t))),
            ),
            TypeDefKind::List(t) => Ty::List(Box::new(ty(resolve, t))),
            TypeDefKind::Handle(Handle::Own(_)) => Ty::Own,
            TypeDefKind::Handle(Handle::Borrow(_)) => Ty::Borrow,
            TypeDefKind::Type(t) => ty(resolve, t),
            other => panic!("{other:?} is outside the subset"),
        },
    }
}

/// The handle table comphost would keep: one per guest-exported resource type.
#[derive(Default)]
pub struct Table {
    reps: Vec<Option<u32>>,
    /// Representations whose handles the guest dropped, for their destructors.
    pub dropped: Vec<u32>,
}

impl Table {
    fn insert(&mut self, rep: u32) -> u32 {
        self.reps.push(Some(rep));
        self.reps.len() as u32
    }

    fn rep(&self, h: u32) -> Option<u32> {
        self.reps.get(h.checked_sub(1)? as usize).copied().flatten()
    }

    fn remove(&mut self, h: u32) -> Option<u32> {
        self.reps.get_mut(h.checked_sub(1)? as usize)?.take()
    }
}

/// The probe guest on wasmi, through `appabi`.
pub struct WasmiGuest {
    store: wasmi::Store<Table>,
    instance: wasmi::Instance,
    memory: wasmi::Memory,
}

impl WasmiGuest {
    pub fn new(wasm: &[u8]) -> Self {
        let mut config = wasmi::Config::default();
        config.compilation_mode(wasmi::CompilationMode::Eager);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, wasm).expect("the guest compiles");
        let mut store = wasmi::Store::new(&engine, Table::default());
        let mut linker = wasmi::Linker::<Table>::new(&engine);
        let resources = format!("[export]{INTERFACE}");
        linker
            .func_wrap(
                &resources,
                "[resource-new]counter",
                |mut c: wasmi::Caller<'_, Table>, rep: i32| c.data_mut().insert(rep as u32) as i32,
            )
            .unwrap()
            .func_wrap(
                &resources,
                "[resource-rep]counter",
                |c: wasmi::Caller<'_, Table>, h: i32| -> Result<i32, wasmi::Error> {
                    c.data()
                        .rep(h as u32)
                        .map(|r| r as i32)
                        .ok_or_else(|| wasmi::Error::new("no such handle"))
                },
            )
            .unwrap()
            .func_wrap(
                &resources,
                "[resource-drop]counter",
                |mut c: wasmi::Caller<'_, Table>, h: i32| -> Result<(), wasmi::Error> {
                    let rep = c
                        .data_mut()
                        .remove(h as u32)
                        .ok_or_else(|| wasmi::Error::new("no such handle"))?;
                    c.data_mut().dropped.push(rep);
                    Ok(())
                },
            )
            .unwrap();
        // Every other function import (`kai.log`, `kai:app/log`, ...) is a no-op
        // that returns zeros: the guests measured here only log.
        for import in module.imports() {
            if let wasmi::ExternType::Func(ty) = import.ty()
                && !(import.module() == resources && import.name().starts_with("[resource-"))
            {
                linker
                    .func_new(
                        import.module(),
                        import.name(),
                        ty.clone(),
                        |_, _, results| {
                            for r in results.iter_mut() {
                                *r = wasmi::Val::I32(0);
                            }
                            Ok(())
                        },
                    )
                    .unwrap();
            }
        }
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .expect("the guest instantiates");
        let memory = instance
            .get_memory(&store, "memory")
            .expect("the guest exports memory");
        Self {
            store,
            instance,
            memory,
        }
    }

    /// Calls `func`, then runs the destructors of handles the guest dropped.
    pub fn invoke(&mut self, func: &Func, args: &[Val]) -> Result<Option<Val>, Trap> {
        let result = appabi_spike::call(self, func, args);
        for rep in std::mem::take(&mut self.store.data_mut().dropped) {
            let dtor = format!("{INTERFACE}#[dtor]counter");
            self.call(&dtor, &[Core::I32(rep as i32)])?;
        }
        result
    }

    /// The host drops an own handle it holds.
    pub fn drop_handle(&mut self, h: u32) -> Result<(), Trap> {
        let rep = self
            .store
            .data_mut()
            .remove(h)
            .ok_or_else(|| Trap("no such handle".into()))?;
        self.call(
            &format!("{INTERFACE}#[dtor]counter"),
            &[Core::I32(rep as i32)],
        )
        .map(drop)
    }

    /// Calls a core export directly, as a hand-bound host does.
    pub fn call_core(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap> {
        self.call(export, args)
    }
}

fn to_wasmi(v: Core) -> wasmi::Val {
    match v {
        Core::I32(i) => wasmi::Val::I32(i),
        Core::I64(i) => wasmi::Val::I64(i),
        Core::F32(f) => wasmi::Val::F32(f.into()),
        Core::F64(f) => wasmi::Val::F64(f.into()),
    }
}

fn from_wasmi(v: &wasmi::Val) -> Core {
    match v {
        wasmi::Val::I32(i) => Core::I32(*i),
        wasmi::Val::I64(i) => Core::I64(*i),
        wasmi::Val::F32(f) => Core::F32(f32::from(*f)),
        wasmi::Val::F64(f) => Core::F64(f64::from(*f)),
        other => panic!("{other:?} is not a number"),
    }
}

impl Guest for WasmiGuest {
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let data = self.memory.data(&self.store);
        let (at, end) = (at as usize, at as usize + len as usize);
        data.get(at..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Trap("a read outside memory".into()))
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        self.memory
            .write(&mut self.store, at as usize, bytes)
            .map_err(|e| Trap(e.to_string()))
    }

    fn realloc(&mut self, old: u32, old_size: u32, align: u32, size: u32) -> Result<u32, Trap> {
        let args = [old, old_size, align, size].map(|n| Core::I32(n as i32));
        match self.call("cabi_realloc", &args)?.as_slice() {
            [Core::I32(at)] if *at != 0 || size == 0 => Ok(*at as u32),
            _ => Err(Trap("cabi_realloc failed".into())),
        }
    }

    fn call(&mut self, export: &str, args: &[Core]) -> Result<Vec<Core>, Trap> {
        let func = self
            .instance
            .get_func(&self.store, export)
            .ok_or_else(|| Trap(format!("no export {export}")))?;
        let params: Vec<wasmi::Val> = args.iter().map(|a| to_wasmi(*a)).collect();
        let count = func.ty(&self.store).results().len();
        let mut results = vec![wasmi::Val::I32(0); count];
        func.call(&mut self.store, &params, &mut results)
            .map_err(|e| Trap(e.to_string()))?;
        Ok(results.iter().map(from_wasmi).collect())
    }

    fn rep(&mut self, h: u32) -> Result<u32, Trap> {
        self.store
            .data()
            .rep(h)
            .ok_or_else(|| Trap("no such handle".into()))
    }
}

/// The same guest bytes, wrapped by wit-component, on Wasmtime 50.
pub struct WasmtimeGuest {
    store: wasmtime::Store<()>,
    instance: wasmtime::component::Instance,
    api: wasmtime::component::ComponentExportIndex,
}

/// The guest module as a component, as wit-component makes it.
pub fn wrap(wasm: &[u8]) -> anyhow::Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .module(wasm)?
        .validate(true)
        .encode()
}

impl WasmtimeGuest {
    pub fn new(component: &[u8]) -> Self {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let component = wasmtime::component::Component::new(&engine, component).unwrap();
        let linker = wasmtime::component::Linker::<()>::new(&engine);
        let mut store = wasmtime::Store::new(&engine, ());
        let instance = linker.instantiate(&mut store, &component).unwrap();
        let api = instance
            .get_export_index(&mut store, None, INTERFACE)
            .expect("the api interface is exported");
        Self {
            store,
            instance,
            api,
        }
    }

    pub fn invoke(
        &mut self,
        name: &str,
        args: &[wasmtime::component::Val],
    ) -> Result<Option<wasmtime::component::Val>, String> {
        let index = self
            .instance
            .get_export_index(&mut self.store, Some(&self.api), name)
            .ok_or_else(|| format!("no export {name}"))?;
        let func = self
            .instance
            .get_func(&mut self.store, index)
            .ok_or_else(|| format!("{name} is not a function"))?;
        let count = func.ty(&self.store).results().len();
        let mut results = vec![wasmtime::component::Val::Bool(false); count];
        func.call(&mut self.store, args, &mut results)
            .map_err(|e| format!("{e:?}"))?;
        Ok(results.pop())
    }

    pub fn drop_handle(&mut self, r: wasmtime::component::ResourceAny) -> Result<(), String> {
        r.resource_drop(&mut self.store)
            .map_err(|e| format!("{e:?}"))
    }
}
