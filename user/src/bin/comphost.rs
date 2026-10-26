//! The component host: runs the live component named like this process (its plan
//! line says `program=comphost`) on wasmi, one request at a time, and swaps versions
//! on `ctl swap` from the kernel. The logic is the host-tested `livecore::host`;
//! this program connects it to the COMP_* calls and the service endpoint.

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
use livecore::settings::Settings;
use user::{arg, args, println, sys};
use wasmhost::{Limits, LiveInstance};

user::entry!(main);

struct SysEnv {
    limits: Limits,
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
    let args = args();
    let settings = match Settings::parse(&args) {
        Ok(settings) => settings,
        Err(why) => {
            println!("bad settings: {why}");
            return 1;
        }
    };
    let mut env = SysEnv {
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
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut caller = u64::MAX;
        match sys::svc_recv(&mut request, host.timeout_ms(), &mut caller) {
            Ok(len) => {
                let len = len.min(request.len());
                let mut reply = match host.handle(caller, &request[..len]) {
                    Ok(reply) => reply,
                    Err(exit) => {
                        println!("{}", exit.why);
                        return exit.code;
                    }
                };
                reply.truncate(kabi::MAX_MESSAGE);
                if let Err(error) = sys::svc_reply(&reply) {
                    println!("svc_reply failed: {error:?}");
                }
            }
            Err(Error::TimedOut) => {
                if let Err(exit) = host.tick() {
                    println!("{}", exit.why);
                    return exit.code;
                }
            }
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
    }
}
