//! The component host: runs the live component named like this process (its plan
//! line says `program=comphost`) on wasmi, one request at a time, and swaps versions
//! on `ctl swap` from the kernel. The logic is the host-tested `livecore::host`;
//! this program connects it to the COMP_* calls and the service endpoint, and,
//! with a route to `desktop`, the app's `kai:ui` windows to the screen (`ui`).
//! An app left unused naps (P3 task 12b): see [`livecore::napper`].

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{MAX_ARTIFACT, MAX_CHECKPOINT, Restore};
use livecore::Fault;
use livecore::host::{Env, Host, Report, Restored};
use livecore::local::restored_of;
use livecore::nap::Nap;
use livecore::napper::{Napped, Napper};
use livecore::settings::Settings;
use sha2::{Digest, Sha256};
use user::{arg, args, println, sys};
use wasmhost::{Limits, LiveInstance};

#[path = "comphost/disk.rs"]
mod disk;
#[path = "comphost/edit.rs"]
mod edit;
#[path = "comphost/files.rs"]
mod files;
#[path = "comphost/settings.rs"]
mod kai_settings;
#[path = "comphost/started.rs"]
mod started;
#[path = "comphost/system.rs"]
mod system;
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
    /// The app's saved state on disk, with a plan line's `dir=apps`; opened
    /// at the first restore, which names the app.
    disk: Option<disk::Disk>,
    /// What the app may import beyond `kai:app` and `kai:ui`: `settings`,
    /// `shell_line` and `system` from the plan line's name for it (the kernel's),
    /// `edit` if the kernel gave this process an `Edit` handle; never from its
    /// package.
    grants: wasmhost::Grants,
    /// The image's own versions fetched, by digest: only they get `grants`.
    image: wasmhost::grants::FromImage,
}

/// The one app whose effects go to the shell as the user's when a person's key
/// or click caused them (the Terminal's `shell_line`): the plan line's name.
const TERMINAL_APP: &str = "terminal-window";
/// The editor's plan line name, which the file door is granted to (`kai:files`).
const EDITOR_APP: &str = "editor";

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
            fetched if fetched == len => {
                self.image.fetched(label, Sha256::digest(&artifact).into());
                Ok(artifact)
            }
            _ => Err(Error::Invalid),
        }
    }

    fn report(&mut self, event: &Report<'_>) -> Result<(), Error> {
        sys::comp_event(&event.to_raw())?;
        // A checkpoint the kernel took is the app's to keep across a reboot.
        if event.kind == kabi::live::kind::CHECKPOINT
            && let Some(disk) = self.disk.as_mut()
        {
            disk.save(event.schema as u32, event.blob);
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<Restored, Error> {
        let mut restore = Restore::empty();
        let mut checkpoint = vec![0u8; MAX_CHECKPOINT];
        let len = sys::comp_restore(&mut restore, &mut checkpoint)?;
        checkpoint.truncate(len.min(MAX_CHECKPOINT));
        let mut restored = restored_of(&restore, &checkpoint);
        if self.disk.is_none() {
            self.disk = disk::Disk::open(&restored.name);
        }
        // After a reboot the kernel has nothing; the disk may.
        let saved = self.disk.as_mut().and_then(|d| d.load(MAX_CHECKPOINT));
        livecore::saved::merge(&mut restored, saved);
        Ok(restored)
    }

    fn load(&mut self, module: &[u8], quiet: bool) -> Result<LiveInstance, Fault> {
        // The sink stays attached; quiet only mutes it, so a standby that becomes
        // the active version again logs as before.
        // The manifest's `lifecycle` picks the contract: 1 hand-bound, 2 kai:live.
        // The grants hold only for the image's own versions: an upload or a
        // package version swapped in under this name runs without them.
        let grants = self
            .image
            .grants_for(&Sha256::digest(module).into(), self.grants);
        if grants != self.grants {
            println!("comphost: this version is not the image's; it runs without its grants");
        }
        let mut instance =
            wasmhost::load_live_granted(module, self.limits, Some(version_log), grants)?;
        livecore::Instance::quiet(&mut instance, quiet);
        // The Settings window is kept from the agent: its rules and grants are
        // typed-only data, and Kai must not park focus on Revoke.
        if let LiveInstance::V2(core) = &mut instance {
            core.allow_agent_private(grants.settings);
        }
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
        disk: None,
        grants: edit::grants(),
        image: wasmhost::grants::FromImage::default(),
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
    // The plan line's name is the grant: `desktop` and the shell check it too.
    env.grants.settings = name == kai_settings::SETTINGS_APP;
    env.grants.shell_line = name == TERMINAL_APP;
    env.grants.files = name == EDITOR_APP;
    if let (true, Some(ui)) = (env.grants.settings, windows.as_mut()) {
        ui.carry_settings();
    }
    // The editor's rules for what it may send; none for an app without the grant.
    let mut edits = env.grants.edit.then(editcore::gate::Gate::new);
    // The plan line's name is the grant: only the System monitor reads the
    // machine, and only it has the route to llm for `health`.
    env.grants.system = name == system::MONITOR_APP;
    let mut source = env.grants.system.then(system::Source::new);
    let mut napper = Napper::new(Nap::new(settings.nap_ms, now_ms()));
    napper.nap.keep_running(settings.keep_running);
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
    settle(&mut windows, &mut edits, &mut host);
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    // Whether the nap holds a lease for settings requests in flight.
    let mut settings_leased = false;
    let mut started = started::Started::default();
    // When the host's own wait ends; 0 is never.
    let mut tick_at = deadline(host.timeout_ms());
    // Whether the nap holds a lease for a read of the machine waiting.
    let mut leased = false;
    loop {
        // The shown instance is told it started, once: a new version or a
        // resumed app is told again (`started.rs`).
        if let Some(performed) = started.tell(&mut host) {
            match windows.as_mut() {
                // Not the person's: no key or click caused it.
                Some(ui) => ui.forward(Some(performed), false),
                None if !performed.is_empty() => {
                    println!("comphost: what the app did as it started has no desktop to go to");
                }
                None => {}
            }
            settle(&mut windows, &mut edits, &mut host);
        }
        let open = windows.as_ref().is_some_and(ui::Ui::wants_poll);
        let wait = match (open, host.timeout_ms()) {
            (false, ms) => ms,
            (true, 0) => ui::POLL_MS,
            (true, ms) => ms.min(ui::POLL_MS),
        };
        // A read waiting for its sample is use: the app does not nap until it
        // has it, and the wait ends when the sample is due.
        let due = source
            .as_ref()
            .and_then(|source| source.wait_ms(&mut host, now_ms()));
        if due.is_some() != leased {
            leased = due.is_some();
            napper.nap.lease(leased);
        }
        let wait = soonest(wait, due);
        let wait = napper.wait(&host, wait, now_ms());
        let mut caller = u64::MAX;
        match sys::svc_recv(&mut request, wait, &mut caller) {
            Ok(len) => {
                let len = len.min(request.len());
                let heap_before = user::heap_bytes();
                let reply = napper
                    .request(&mut host, caller, &request[..len], now_ms())
                    .and_then(|(reply, napped)| {
                        log_nap(&name, napped);
                        match reply {
                            Some(reply) => Ok(reply),
                            None => host.handle(caller, &request[..len]),
                        }
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
                napper.after(&host, now_ms());
                settle(&mut windows, &mut edits, &mut host);
            }
            Err(Error::TimedOut) => {
                if let Some(ui) = windows.as_mut().filter(|ui| ui.wants_poll())
                    && let Err(exit) = ui
                        .poll(&mut host)
                        .and_then(|()| {
                            // A close or an agent's action may have woken it.
                            napper.after(&host, now_ms());
                            napper.focus(&mut host, ui.take_focused(), now_ms())
                        })
                        .map(|napped| log_nap(&name, napped))
                {
                    println!("{}", exit.why);
                    return exit.code;
                }
                // A settings request waiting for its answer is use: the app
                // does not nap until it has it.
                let busy = windows.as_ref().is_some_and(ui::Ui::settings_busy);
                if busy != settings_leased {
                    napper.nap.lease(busy);
                    settings_leased = busy;
                }
                settle(&mut windows, &mut edits, &mut host);
                match napper.tick(&mut host, now_ms()) {
                    Ok(napped) => log_nap(&name, napped),
                    Err(exit) => {
                        println!("{}", exit.why);
                        return exit.code;
                    }
                }
                if tick_at != 0 && now_ms() >= tick_at {
                    if let Err(exit) = host.tick() {
                        println!("{}", exit.why);
                        return exit.code;
                    }
                    tick_at = deadline(host.timeout_ms());
                    settle(&mut windows, &mut edits, &mut host);
                }
            }
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
        // A read of the machine that is due is answered, and what the app drew
        // for the sample is shown.
        if let Some(source) = source.as_mut()
            && source.pump(&mut host, now_ms())
        {
            settle(&mut windows, &mut edits, &mut host);
        }
    }
}

/// The shorter of the host's wait (0 is never) and the time until a read is due
/// (at least 1, since 0 would mean never).
fn soonest(wait: u64, due: Option<u64>) -> u64 {
    match due {
        None => wait,
        Some(ms) if wait == 0 => ms.max(1),
        Some(ms) => wait.min(ms.max(1)),
    }
}

/// What the instances did that leaves comphost: the editor's messages go to the
/// edit service, then the windows' frames to `desktop`.
fn settle(
    windows: &mut Option<ui::Ui>,
    edits: &mut Option<editcore::gate::Gate>,
    host: &mut Host<SysEnv>,
) {
    if let Some(gate) = edits.as_mut() {
        edit::pump(host, gate);
        files::pump(host);
    }
    if let Some(ui) = windows.as_mut() {
        ui.pump(host);
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

/// The nap's log lines.
fn log_nap(name: &str, napped: Option<Napped>) {
    match napped {
        Some(Napped::Napped) => println!("comphost: {name} napped, unused"),
        Some(Napped::Woke(reply)) => println!(
            "comphost: woke from its nap: {}",
            alloc::string::String::from_utf8_lossy(&reply)
        ),
        None => {}
    }
}
