//! `trial <component>` (`kai-cc 1`; #583's piece 3b): a built app run once on
//! stub imports before anyone is asked to install it, as ADR 0012 D5 runs a
//! model's program on stubs first. It comes with the `Shm` its `build` left
//! the artifact in.
//!
//! cc loads the artifact on wasmhost with no grants and the limits comphost
//! gives an app by default ([`limits`]), then runs:
//! 1. `init`, and the check comphost makes after it: the app's state must say
//!    its own schema;
//! 2. `started`, if the app exports `kai:app/events`;
//! 3. for an app that opened a window: a key to it, then a click on its first
//!    button, as the agent's `ui_act` clicks.
//!
//! Each event is one transaction, checked as livecore's host checks one. What
//! it queues is counted and dropped, never run; its storage reads see nothing,
//! and its clock is fixed. Each step must end within one line budget, and
//! Ctrl-C stops the trial between steps. An app that imports `kai:ui/view` but
//! opens no window fails. A failure's text names the step first, for the
//! repair loop, within `err`'s 2 KiB. One that is not the program's fault
//! (the `Shm`, the artifact cc wrote, wasmhost refusing it for a reason of its
//! own, or Ctrl-C) starts [`INTERNAL`], so the shell reports it instead of
//! spending a repair on it.

use crate::Host;
use crate::build::shm;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::live::MAX_ARTIFACT;
use livecore::settings::Settings;
use livecore::txn::{Inputs, Storage};
use livecore::{Fault, Instance, Reason};
use uikit::Frame;
use uikit::a11y::{Action, Role};
use uikit::input::{Input, Key, Mods};
use wasmhost::{AppEvent, CoreInstance, Grants, Limits, LiveInstance, Update};
use wasmparser::{Parser, Payload};

/// comphost's default limits for an app (`livecore::Settings::DEFAULT`).
pub fn limits() -> Limits {
    let settings = Settings::DEFAULT;
    Limits {
        fuel_call: settings.fuel_call,
        fuel_life: settings.fuel_life,
        memory_bytes: usize::try_from(settings.memory_kib)
            .unwrap_or(usize::MAX)
            .saturating_mul(1024),
    }
}

/// How a failure that is not the program's fault starts.
pub const INTERNAL: &str = "internal: ";

/// The interface a window app imports.
const VIEW: &str = "kai:ui/view@";
/// HID usage 0x04, the A key.
const KEY_A: u16 = 0x04;
/// The clock and seed every call of a trial sees.
const NOW_MS: u64 = 1_700_000_000_000;
const SEED: u64 = 0x6b61_6974_7269_616c;

/// Runs the artifact in `shm` as `component`'s trial. `Ok` is its one-line
/// report, `Err` why it failed, its step first.
pub fn serve(component: &str, shm: &[u8], host: &dyn Host) -> Result<String, String> {
    // A copy: the shell could write the `Shm` while cc reads it.
    let artifact = shm::take(shm, MAX_ARTIFACT)
        .map_err(|why| internal(&format!("the trial's artifact: {why}")))?
        .to_vec();
    check(&artifact, component).map_err(|why| internal(&why))?;
    let app = match wasmhost::load_live_granted(&artifact, limits(), None, Grants::default()) {
        Ok(LiveInstance::V2(app)) => app,
        Ok(LiveInstance::V1(_)) => return Err(internal("it is not a lifecycle 2 app")),
        Err(fault) => return Err(refused(&fault)),
    };
    let mut trial = Trial::new(host, app)?;
    trial.init()?;
    trial.call("started", |app| app.deliver_event(&AppEvent::Started))?;
    let Some(window) = trial.app.ui_windows().first().copied() else {
        return match imports_view(&artifact) {
            true => Err(String::from(
                "it opened no window: init or started must open one and commit it",
            )),
            false => Ok(trial.report()),
        };
    };
    let key = Input::Key {
        key: Key::Other(KEY_A),
        down: true,
        mods: Mods::default(),
        text: String::from("a"),
    };
    trial.call("a key", |app| app.ui_input(window, key))?;
    let shown = trial.last.as_ref().filter(|(shown, _)| *shown == window);
    if let Some((node, name)) = shown.and_then(|(_, frame)| button(frame)) {
        let what = format!("a click on node {node} ({name:?})");
        trial.call(&what, |app| {
            app.ui_act(window, node, Action::Click, None).map(|_| ())
        })?;
    }
    Ok(trial.report())
}

/// The artifact must be `component`'s, of the app world, on `kai:live`.
fn check(artifact: &[u8], component: &str) -> Result<(), String> {
    let parsed = livecore::artifact::parse(artifact)
        .map_err(|why| format!("the trial's artifact does not load: {why}"))?;
    let manifest = &parsed.manifest;
    let world = format!("app:{component}/{component}@");
    if manifest.component != component || !manifest.world.starts_with(&world) {
        return Err(format!(
            "the artifact is {}, not {component}'s",
            manifest.world
        ));
    }
    if manifest.lifecycle != 2 {
        return Err(String::from("it is not a lifecycle 2 app"));
    }
    Ok(())
}

/// The interfaces the module in `artifact` imports, each once, in order
/// (`kai:ui/view@0.1.0`): what an app's install card lists. Empty for one that
/// does not parse.
pub fn imports(artifact: &[u8]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for payload in Parser::new(0).parse_all(artifact) {
        let Ok(Payload::ImportSection(reader)) = payload else {
            continue;
        };
        for import in reader.into_iter().flatten() {
            if !seen.iter().any(|s| s == import.module) {
                seen.push(import.module.to_string());
            }
        }
    }
    seen
}

/// Whether the module imports `kai:ui/view`: a window app.
fn imports_view(artifact: &[u8]) -> bool {
    Parser::new(0)
        .parse_all(artifact)
        .any(|payload| match payload {
            Ok(Payload::ImportSection(reader)) => reader
                .into_iter()
                .any(|import| import.is_ok_and(|i| i.module.starts_with(VIEW))),
            _ => false,
        })
}

/// The first button in `frame` that takes a click, and its name.
fn button(frame: &Frame) -> Option<(u32, String)> {
    frame
        .a11y
        .iter()
        .find(|node| node.role == Role::Button && node.actions & Action::Click.bit() != 0)
        .map(|node| (node.id, node.name.clone()))
}

/// `what` failed with `fault`, the program's fault: a call's fault is, even a
/// host one, such as a string it passed that is not UTF-8.
fn failed(what: &str, fault: &Fault) -> String {
    format!("{what} failed ({}): {}", fault.reason.name(), fault.detail)
}

/// wasmhost did not load the artifact: internal when it refused it for a
/// reason of its own, the program's when it ran out of memory or fuel
/// instantiating it.
fn refused(fault: &Fault) -> String {
    let text = failed("loading it", fault);
    match fault.reason {
        Reason::Host | Reason::Manifest | Reason::Compile | Reason::Link => internal(&text),
        _ => text,
    }
}

/// `text`, marked as no fault of the program's.
fn internal(text: &str) -> String {
    format!("{INTERNAL}{text}")
}

/// One app's trial: what it ran and what it saw.
struct Trial<'h> {
    host: &'h dyn Host,
    app: CoreInstance,
    schema: u32,
    storage: Storage,
    steps: Vec<String>,
    frames: usize,
    effects: usize,
    /// The newest frame, and its window.
    last: Option<(u32, Frame)>,
}

impl<'h> Trial<'h> {
    fn new(host: &'h dyn Host, mut app: CoreInstance) -> Result<Self, String> {
        let storage = Storage::new();
        app.view(&storage);
        let schema = app.schema().map_err(|fault| failed("its schema", &fault))?;
        Ok(Self {
            host,
            app,
            schema,
            storage,
            steps: Vec::new(),
            frames: 0,
            effects: 0,
            last: None,
        })
    }

    /// `init`, outside a transaction, as comphost starts an app.
    fn init(&mut self) -> Result<(), String> {
        let app = &mut self.app;
        let code = timed(self.host, "init", || app.init())?;
        if code != 0 {
            return Err(format!("init failed: it returned {code}"));
        }
        self.after("init")
    }

    /// `run` as one transaction, checked as livecore checks a call's.
    fn call(
        &mut self,
        what: &str,
        run: impl FnOnce(&mut CoreInstance) -> Result<(), Fault>,
    ) -> Result<(), String> {
        let inputs = Inputs {
            now_ms: NOW_MS,
            seed: SEED,
            first_ticket: u32::try_from(self.effects).unwrap_or(u32::MAX),
        };
        let (app, storage) = (&mut self.app, &self.storage);
        let txn = timed(self.host, what, || {
            app.begin(storage, inputs);
            let ran = run(app);
            let txn = app.finish();
            ran?;
            livecore::host::check_txn(&txn, inputs, 0)?;
            Ok(txn)
        })?;
        self.effects += txn.effects.len();
        self.after(what)
    }

    /// After `what`: the state check, and the frames it committed.
    fn after(&mut self, what: &str) -> Result<(), String> {
        let state = self
            .app
            .export(0)
            .map_err(|fault| failed(&format!("its state after {what}"), &fault))?
            .ok_or_else(|| format!("after {what}, it cannot export its own state"))?;
        match livecore::state::schema_of(&state) {
            Some(schema) if schema == u64::from(self.schema) => {}
            found => {
                return Err(format!(
                    "after {what}, its state says schema {found:?}, not {}",
                    self.schema
                ));
            }
        }
        for update in self.app.take_ui() {
            if let Update::Frame { window, frame } = update {
                self.frames += 1;
                self.last = Some((window, frame));
            }
        }
        self.steps.push(what.to_string());
        Ok(())
    }

    fn report(&self) -> String {
        let windows = self.app.ui_windows().len();
        format!(
            "trial ok: {}; {windows} window(s), {} frame(s); {} effect(s) queued, none run",
            self.steps.join(", "),
            self.frames,
            self.effects
        )
    }
}

/// Runs `step` after a Ctrl-C check; it must end within one line budget.
fn timed<R>(
    host: &dyn Host,
    what: &str,
    step: impl FnOnce() -> Result<R, Fault>,
) -> Result<R, String> {
    if host.interrupted() {
        return Err(internal(&format!("the trial was stopped before {what}")));
    }
    let start = host.now_ns();
    let out = step().map_err(|fault| failed(what, &fault))?;
    let spent = host.now_ns().saturating_sub(start);
    if spent > host.line_ns() {
        return Err(format!(
            "{what} took {} ms, over the {} ms one step may take",
            spent / 1_000_000,
            host.line_ns() / 1_000_000
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_budget_counts_the_memory_a_trial_gives() {
        assert_eq!(limits().memory_bytes as u64, crate::budget::TRIAL_MEMORY);
    }

    #[test]
    fn only_wasmhosts_own_refusals_are_internal() {
        for reason in [
            Reason::Host,
            Reason::Manifest,
            Reason::Compile,
            Reason::Link,
        ] {
            let text = refused(&Fault::new(reason, "no"));
            assert!(text.starts_with(INTERNAL), "{text}");
        }
        for reason in [Reason::Memory, Reason::Fuel] {
            let text = refused(&Fault::new(reason, "no"));
            assert!(text.starts_with("loading it failed ("), "{text}");
        }
        // A call's fault is the program's, a host one included.
        for reason in [
            Reason::Trap,
            Reason::Fuel,
            Reason::Memory,
            Reason::Abi,
            Reason::Host,
        ] {
            let text = failed("init", &Fault::new(reason, "no"));
            assert!(text.starts_with("init failed ("), "{text}");
        }
    }

    #[test]
    fn only_a_module_that_imports_view_is_a_window_app() {
        let window = module_importing("kai:ui/view@0.1.1", "open");
        assert!(imports_view(&window));
        let plain = module_importing("kai:app/log@0.1.0", "write");
        assert!(!imports_view(&plain));
        assert!(!imports_view(b"\0asm"));
        assert!(!imports_view(b"not a module"));
    }

    /// A module with one function import `module`.`name`, of type `() -> ()`,
    /// written out byte by byte.
    fn module_importing(module: &str, name: &str) -> Vec<u8> {
        let mut imports = vec![1u8];
        for text in [module, name] {
            imports.push(u8::try_from(text.len()).unwrap());
            imports.extend_from_slice(text.as_bytes());
        }
        imports.extend_from_slice(&[0x00, 0x00]);
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.extend_from_slice(&[0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
        out.push(0x02);
        out.push(u8::try_from(imports.len()).unwrap());
        out.extend_from_slice(&imports);
        out
    }
}
