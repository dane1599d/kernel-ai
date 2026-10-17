//! The M9a scenario on the host before QEMU boots: the real versions on wasmi, the
//! component host from livecore, and the store and ledger standing in for the
//! kernel. A failure here points at the host or a version, not at the kernel.

use crate::esp::Module;
use livecore::host::{Exit, Host};
use livecore::local::{Kernel, Loader, LocalEnv};
use livecore::scenario::{self, Driver};
use livecore::settings::Settings;
use livecore::store::{Bytes, Component};
use wasmhost::{Limits, WasmInstance};

/// The settings the kernel's test plan gives the host.
pub fn test_settings() -> Result<Settings, String> {
    let plan = supervise::parse_plan(kabi::plan::NOTES_TEST);
    let notes = plan
        .services
        .first()
        .ok_or("the notes test plan does not parse")?;
    Settings::parse(&notes.args)
}

fn print_log(line: &str) {
    println!("  | [notes] {line}");
}

pub fn loader(settings: &Settings) -> Loader<WasmInstance> {
    let limits = Limits {
        fuel_call: settings.fuel_call,
        fuel_life: settings.fuel_life,
        memory_bytes: settings.memory_kib as usize * 1024,
    };
    Box::new(move |module, quiet| {
        let mut instance = wasmhost::load(module, limits, Some(print_log))?;
        livecore::Instance::quiet(&mut instance, quiet);
        Ok(instance)
    })
}

struct Smoke {
    kernel: Kernel,
    settings: Settings,
    pid: u64,
    host: Option<Host<LocalEnv<WasmInstance>>>,
}

fn failed(exit: Exit) -> String {
    format!("the host exited ({}): {}", exit.code, exit.why)
}

impl Smoke {
    fn start(&mut self) -> Result<(), String> {
        let env = self.kernel.env(self.pid, loader(&self.settings));
        self.host = Some(Host::start(env, "notes", self.settings.clone()).map_err(failed)?);
        Ok(())
    }

    fn send(&mut self, caller: u64, request: &str) -> Result<String, String> {
        let host = self.host.as_mut().ok_or("no host is running")?;
        let reply = host.handle(caller, request.as_bytes()).map_err(failed)?;
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }
}

impl Driver for Smoke {
    fn call(&mut self, request: &str) -> Result<String, String> {
        self.send(0, request)
    }

    fn spoof(&mut self, request: &str) -> Result<String, String> {
        self.send(4242, request)
    }

    fn ledger(&mut self) -> livecore::ledger::Ledger {
        self.kernel.component.borrow().ledger.clone()
    }

    fn fetches(&mut self, label: &str) -> u64 {
        self.kernel.component.borrow().fetches(label)
    }

    fn host_pid(&mut self) -> u64 {
        self.pid
    }

    fn restart(&mut self) -> Result<(), String> {
        self.host = None;
        self.pid += 1;
        self.start()
    }

    fn log(&mut self, text: &str) {
        println!("xtask: {text}");
    }
}

/// Runs the scenario on the `notes` versions among `modules`.
pub fn run(modules: &[Module]) -> Result<(), String> {
    let mut component = Component::new("notes");
    for module in modules {
        if let Some(label) = module
            .path
            .strip_prefix("comp/notes/")
            .and_then(|p| p.strip_suffix(".wasm"))
        {
            component.add_version(label, Bytes::shared(module.bytes.clone()))?;
        }
    }
    let mut smoke = Smoke {
        kernel: Kernel::new(component),
        settings: test_settings()?,
        pid: 3,
        host: None,
    };
    smoke.start()?;
    let started = std::time::Instant::now();
    let result = scenario::run(&mut smoke);
    for line in smoke.kernel.log.borrow().iter() {
        println!("  | {line}");
    }
    result?;
    println!(
        "xtask: the live scenario passed on the host ({} steps, {} ms)",
        scenario::STEPS.len(),
        started.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_plan_gives_the_host_calls_only_probation() {
        let settings = test_settings().unwrap();
        assert_eq!(
            (
                settings.version.as_str(),
                settings.probation_calls,
                settings.probation_ms
            ),
            ("1", 3, 0)
        );
    }

    /// xtask is built without optimisation; wasmi's host-function glue must still
    /// work when reached through wasmhost from here.
    #[test]
    fn a_host_import_works_when_called_from_an_unoptimised_crate() {
        let wat = r#"(module
          (import "kai" "log" (func $log (param i32 i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "ok")
          (func (export "kai_alloc") (param i32) (result i32) (i32.const 1024))
          (func (export "kai_schema") (result i32) (i32.const 1))
          (func (export "kai_init") (result i32) (i32.const 0))
          (func (export "kai_import") (param i32 i32 i32) (result i32) (i32.const 0))
          (func (export "kai_export") (param i32) (result i64) (i64.const -1))
          (func (export "kai_handle") (param i32 i32) (result i64)
            (call $log (i32.const 16) (i32.const 2))
            (i64.const 0x0000001000000002))
          (func (export "kai_health") (result i32) (i32.const 0))
          (func (export "kai_self_test") (result i32) (i32.const 0)))"#;
        let module = wat::parse_str(wat).unwrap();
        let limits = Limits {
            fuel_call: 1000,
            fuel_life: 1000,
            memory_bytes: 1 << 20,
        };
        let mut instance = wasmhost::load(&module, limits, Some(print_log)).unwrap();
        assert_eq!(
            livecore::Instance::handle(&mut instance, b"x").unwrap(),
            b"ok"
        );
    }
}
