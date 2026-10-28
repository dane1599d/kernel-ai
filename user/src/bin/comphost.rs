//! The component host: runs the live component named like this process (its plan
//! line says `program=comphost`) on wasmi, one request at a time, and swaps versions
//! on `ctl swap` from the kernel. The logic is the host-tested `livecore::host`;
//! this program connects it to the COMP_* calls and the service endpoint, and,
//! with a route to `desktop`, the app's `kai:ui` windows to the screen (`ui`).
//! An app left unused naps (P3 task 12b): see [`Napper`].

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{MAX_ARTIFACT, MAX_CHECKPOINT, Restore};
use livecore::Fault;
use livecore::host::{Env, Exit, Host, Report, Restored};
use livecore::local::restored_of;
use livecore::nap::Nap;
use livecore::settings::Settings;
use user::{arg, args, println, sys};
use wasmhost::{Limits, LiveInstance};

#[path = "comphost/ui.rs"]
mod ui;

user::entry!(main);

/// Each instance's linear memory in a reservation of its own (P3 task 11):
/// committed as it grows, released when the instance goes, so a suspended
/// app's pages go back to the kernel.
static MEM: wasmhost::linear::Pages = wasmhost::linear::Pages {
    reserve: |bytes| {
        let at = sys::mem_reserve(bytes as u64).ok()?;
        core::ptr::NonNull::new(at as *mut u8)
    },
    commit: |at, bytes| sys::mem_commit(at.as_ptr() as u64, bytes as u64).is_ok(),
    release: |at, _| {
        let _ = sys::mem_release(at.as_ptr() as u64);
    },
};

struct SysEnv {
    limits: Limits,
    /// Whether the app's windows reach `desktop`.
    windows: bool,
}

fn version_log(line: &str) {
    println!("{line}");
}

impl Env for SysEnv {
    type Instance = LiveInstance;

    fn fetch(&mut self, label: &str) -> Result<Vec<u8>, Error> {
        // The length first (a fetch that does not fit is not counted), then the bytes.
        let len = sys::comp_fetch(label, &mut [])?;
        if len > MAX_ARTIFACT {
            return Err(Error::TooBig);
        }
        let mut artifact = vec![0u8; len];
        match sys::comp_fetch(label, &mut artifact)? {
            fetched if fetched == len => Ok(artifact),
            _ => Err(Error::Invalid),
        }
    }

    fn report(&mut self, event: &Report<'_>) -> Result<(), Error> {
        sys::comp_event(&event.to_raw())
    }

    fn restore(&mut self) -> Result<Restored, Error> {
        let mut restore = Restore::empty();
        let mut checkpoint = vec![0u8; MAX_CHECKPOINT];
        let len = sys::comp_restore(&mut restore, &mut checkpoint)?;
        checkpoint.truncate(len.min(MAX_CHECKPOINT));
        Ok(restored_of(&restore, &checkpoint))
    }

    fn load(&mut self, module: &[u8], quiet: bool) -> Result<LiveInstance, Fault> {
        // The sink stays attached; quiet only mutes it, so a standby that becomes
        // the active version again logs as before.
        // The manifest's `lifecycle` picks the contract: 1 hand-bound, 2 kai:live.
        let mut instance = wasmhost::load_live(module, self.limits, Some(version_log))?;
        livecore::Instance::quiet(&mut instance, quiet);
        // Its windows are laid out with the fonts they are drawn with.
        if let (LiveInstance::V2(core), true) = (&mut instance, self.windows)
            && let Some(measure) = ui::measure()
        {
            core.set_measure(measure);
        }
        Ok(instance)
    }

    fn now_ms(&self) -> u64 {
        sys::time_ns() / 1_000_000
    }

    fn log(&mut self, text: &str) {
        println!("{text}");
    }
}

fn main() -> i32 {
    wasmhost::linear::use_pages(&MEM);
    let args = args();
    let settings = match Settings::parse(&args) {
        Ok(settings) => settings,
        Err(why) => {
            println!("bad settings: {why}");
            return 1;
        }
    };
    let mut windows = ui::Ui::new();
    let mut env = SysEnv {
        windows: windows.is_some(),
        limits: Limits {
            fuel_call: settings.fuel_call,
            fuel_life: settings.fuel_life,
            memory_bytes: settings.memory_kib as usize * 1024,
        },
    };
    let name = match env.restore() {
        Ok(restored) => restored.name,
        Err(error) => {
            println!("cannot read the ledger: {error:?}");
            return livecore::host::EXIT_REFUSED;
        }
    };
    let mut napper = Napper {
        nap: Nap::new(settings.nap_ms, now_ms()),
    };
    let mut host = match Host::start(env, &name, settings) {
        Ok(host) => host,
        Err(exit) => {
            println!("{}", exit.why);
            return exit.code;
        }
    };
    println!(
        "up (restart {}): serving {name}",
        arg(&args, "restart").unwrap_or("0")
    );
    if let Some(ui) = windows.as_mut() {
        ui.pump(&mut host);
    }
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    // When the host's own wait ends; 0 is never.
    let mut tick_at = deadline(host.timeout_ms());
    loop {
        let open = windows.as_ref().is_some_and(ui::Ui::has_windows);
        let wait = match (open, host.timeout_ms()) {
            (false, ms) => ms,
            (true, 0) => ui::POLL_MS,
            (true, ms) => ms.min(ui::POLL_MS),
        };
        let wait = napper.wait(&host, wait);
        let mut caller = u64::MAX;
        match sys::svc_recv(&mut request, wait, &mut caller) {
            Ok(len) => {
                let len = len.min(request.len());
                let heap_before = user::heap_bytes();
                let reply = napper
                    .request(&mut host, caller, &request[..len])
                    .and_then(|reply| match reply {
                        Some(reply) => Ok(reply),
                        None => host.handle(caller, &request[..len]),
                    });
                let mut reply = match reply {
                    Ok(reply) => reply,
                    Err(exit) => {
                        println!("{}", exit.why);
                        return exit.code;
                    }
                };
                note_heap(&mut reply, heap_before);
                reply.truncate(kabi::MAX_MESSAGE);
                if let Err(error) = sys::svc_reply(&reply) {
                    println!("svc_reply failed: {error:?}");
                }
                tick_at = deadline(host.timeout_ms());
                napper.after(&host);
                if let Some(ui) = windows.as_mut() {
                    ui.pump(&mut host);
                }
            }
            Err(Error::TimedOut) => {
                if let Some(ui) = windows.as_mut().filter(|ui| ui.has_windows())
                    && let Err(exit) = ui
                        .poll(&mut host)
                        .and_then(|()| napper.focus(&mut host, ui.take_focused()))
                {
                    println!("{}", exit.why);
                    return exit.code;
                }
                if let Err(exit) = napper.tick(&mut host, &name) {
                    println!("{}", exit.why);
                    return exit.code;
                }
                if tick_at != 0 && now_ms() >= tick_at {
                    if let Err(exit) = host.tick() {
                        println!("{}", exit.why);
                        return exit.code;
                    }
                    tick_at = deadline(host.timeout_ms());
                    if let Some(ui) = windows.as_mut() {
                        ui.pump(&mut host);
                    }
                }
            }
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
    }
}

/// After a suspend or a resume (P3 task 11), the heap in use before and after
/// joins the reply and the log, so what the instance held is seen to go.
fn note_heap(reply: &mut Vec<u8>, before: u64) {
    let moved = [b"ok suspended ".as_slice(), b"ok resumed ".as_slice()];
    if !moved.iter().any(|ok| reply.starts_with(ok)) {
        return;
    }
    let (before, after) = (before / 1024, user::heap_bytes() / 1024);
    let note = alloc::format!("; heap {before} KiB -> {after} KiB");
    reply.extend_from_slice(note.as_bytes());
    println!("{}", core::str::from_utf8(reply).unwrap_or(""));
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

/// When a wait of `ms` started now ends; 0 for none.
fn deadline(ms: u64) -> u64 {
    if ms == 0 { 0 } else { now_ms() + ms }
}

/// App nap (P3 task 12b): the app naps after `nap_ms` unused (no request, no
/// focus) unless its keep-running switch or a background-work lease holds it,
/// and wakes on its next request or focus. comphost wakes only what it napped:
/// a standing instruction's suspend keeps its authority until its `until`.
struct Napper {
    nap: Nap,
}

/// The keep-running switch, from the kernel only: `ctl keep-running on|off`.
const KEEP_RUNNING: &str = "ctl keep-running ";

impl Napper {
    /// How long to wait, at most `wait` (0 forever), so a nap comes on time.
    fn wait<E: Env>(&self, host: &Host<E>, wait: u64) -> u64 {
        let nap = (!host.suspended())
            .then(|| self.nap.wait_ms(now_ms()))
            .flatten()
            .map(|ms| ms.max(1));
        match (wait, nap) {
            (0, Some(nap)) => nap,
            (wait, Some(nap)) => wait.min(nap),
            (wait, None) => wait,
        }
    }

    /// Before a request: the switch is answered here; anything else is use,
    /// and wakes a napping app first, so even `ctl swap` finds it running.
    fn request<E: Env>(
        &mut self,
        host: &mut Host<E>,
        caller: u64,
        request: &[u8],
    ) -> Result<Option<Vec<u8>>, Exit> {
        if caller == 0
            && let Some(on) = request.strip_prefix(KEEP_RUNNING.as_bytes())
        {
            let reply: &[u8] = match on {
                b"on" => b"ok keep-running on",
                b"off" => b"ok keep-running off",
                _ => b"err usage: ctl keep-running on|off",
            };
            if on == b"on" || on == b"off" {
                self.nap.keep_running(on == b"on");
            }
            return Ok(Some(reply.to_vec()));
        }
        self.nap.used(now_ms());
        self.wake(host)?;
        Ok(None)
    }

    /// After a request: a resume from any side ends the nap.
    fn after<E: Env>(&mut self, host: &Host<E>) {
        if self.nap.is_napping() && !host.suspended() {
            self.nap.woke(now_ms());
        }
    }

    /// One of its windows took focus: use, which wakes its nap.
    fn focus<E: Env>(&mut self, host: &mut Host<E>, focused: bool) -> Result<(), Exit> {
        if focused {
            self.nap.used(now_ms());
            self.wake(host)?;
        }
        Ok(())
    }

    fn wake<E: Env>(&mut self, host: &mut Host<E>) -> Result<(), Exit> {
        if self.nap.is_napping()
            && let Some(reply) = host.wake()?
        {
            println!(
                "comphost: woke from its nap: {}",
                alloc::string::String::from_utf8_lossy(&reply)
            );
            self.nap.woke(now_ms());
        }
        Ok(())
    }

    /// When the wait ends: nap if it is due and the app serves alone.
    fn tick<E: Env>(&mut self, host: &mut Host<E>, name: &str) -> Result<(), Exit> {
        if host.suspended() || !self.nap.due(now_ms()) {
            return Ok(());
        }
        let request = alloc::format!("{}suspend", kabi::live::CONTROL_PREFIX);
        let reply = host.handle(0, request.as_bytes())?;
        if reply.starts_with(b"ok suspended") {
            self.nap.napped();
            println!("comphost: {name} napped, unused");
        } else {
            // On probation or unsaved: try again a whole nap later.
            self.nap.used(now_ms());
        }
        Ok(())
    }
}
