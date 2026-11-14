//! A window app built and started as the OS does it: `cc build` on the
//! `window` world, then started in wasmhost as comphost starts a version, with
//! comphost's default limits. The `c-app-1` card's tests and the app gate
//! ([`crate::appgate`]) share it.

use ccserve::build::{self, Build, BuildError, Built, Kind};
use ccserve::{Budget, Compiler, Host};
use livecore::Instance;
use livecore::txn::{Inputs, Storage};
use std::sync::OnceLock;
use uikit::a11y::A11yNode;
use wasmhost::{CoreInstance, Limits, LiveInstance, Update};

/// What comphost gives a version by default (`livecore::settings::Settings::DEFAULT`),
/// so what runs here runs in the OS.
pub fn limits() -> Limits {
    let os = livecore::settings::Settings::DEFAULT;
    Limits {
        fuel_call: os.fuel_call,
        fuel_life: os.fuel_life,
        memory_bytes: os.memory_kib as usize * 1024,
    }
}

pub struct Clock;

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        0
    }

    fn interrupted(&self) -> bool {
        false
    }
}

fn compiler() -> &'static Compiler {
    static COMPILER: OnceLock<Compiler> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let cc = Compiler::new(ccserve::CC_WASM, &[]).expect("cc.wasm");
        build::compiler(&cc).expect("headers")
    })
}

/// `main` built as component `name`, of the kind its includes make it, as
/// `cc build` builds one file.
pub fn build_main(name: &str, version: &str, main: &str) -> Result<(Kind, Built), BuildError> {
    let files = [("main.c", main.as_bytes())];
    let kind = Kind::of(&files);
    let mut budget = Budget::new(&Clock, u64::MAX / 4);
    let build = Build {
        name,
        version,
        kind,
        files: &files,
    };
    build::build(compiler(), &build, &mut budget).map(|built| (kind, built))
}

/// `cc trial` of `artifact` as component `name`: its one-line report, or why
/// it failed, its step first.
pub fn trial(name: &str, artifact: &[u8]) -> Result<String, String> {
    let mut shm = vec![0u8; build::shm::BYTES];
    build::shm::put(&mut shm, artifact).map_err(|why| format!("internal: the Shm: {why}"))?;
    ccserve::trial::serve(name, &shm, &Clock)
}

/// `f` inside a call's transaction, as comphost runs an event.
pub fn in_call<R>(app: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> R {
    let inputs = Inputs {
        now_ms: 0,
        seed: 0,
        first_ticket: 0,
    };
    app.begin(&Storage::new(), inputs);
    let result = f(app);
    app.finish();
    result
}

/// `artifact` loaded as a lifecycle 2 app.
pub fn load(artifact: &[u8]) -> Result<CoreInstance, String> {
    match wasmhost::load_live(artifact, limits(), None) {
        Ok(LiveInstance::V2(app)) => Ok(app),
        Ok(LiveInstance::V1(_)) => Err(String::from("a lifecycle 1 artifact")),
        Err(why) => Err(format!("it does not load: {why:?}")),
    }
}

/// The app's own state, as comphost exports it.
pub fn saved(app: &mut CoreInstance) -> Result<Vec<u8>, String> {
    match app.export(0) {
        Ok(Some(state)) => Ok(state),
        Ok(None) => Err(String::from("it has no state")),
        Err(why) => Err(format!("its state does not export: {why:?}")),
    }
}

/// `artifact` started as comphost starts a version: `init` outside any call's
/// transaction, then its own state exported and its schema read
/// (`livecore::host::export_own`). Its window's id.
pub fn start(artifact: &[u8]) -> Result<(CoreInstance, u32), String> {
    let mut app = load(artifact)?;
    match app.init() {
        Ok(0) => {}
        other => return Err(format!("init answered {other:?}")),
    }
    if livecore::state::schema_of(&saved(&mut app)?) != Some(1) {
        return Err(String::from("its state is not schema 1"));
    }
    match app.ui_windows().as_slice() {
        [window] => Ok((app, *window)),
        windows => Err(format!("{} windows, not one", windows.len())),
    }
}

/// The nodes of the last frame the app sent, if it sent one.
pub fn last_frame(app: &mut CoreInstance) -> Option<Vec<A11yNode>> {
    app.take_ui()
        .into_iter()
        .filter_map(|u| match u {
            Update::Frame { frame, .. } => Some(frame.a11y),
            _ => None,
        })
        .next_back()
}
