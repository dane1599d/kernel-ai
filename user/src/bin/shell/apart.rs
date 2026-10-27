//! Scripts in a `scriptrun` child (P1 task 14): the shell claims one from its
//! pool, moves in only a run-tier `Sched` handle and the `Shm` that carries the
//! request and the answer, and waits for it on the script's time limit. Ctrl-C
//! or the limit ends the child with JOB_KILL; a killed child leaves the pool for
//! good and the kernel starts its replacement on the next claim.

use alloc::format;
use alloc::string::String;
use core::cell::Cell;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kabi::{ExitReason, ExitRecord};
use user::sys;

/// Pages of the `Shm` a request and its answer share: room for the longest
/// script and output the shell keeps, and the changes after it.
const PAGES: u64 = 8;
/// How many idle children the pool keeps: one script runs at a time.
const POOL: u64 = 1;
/// How often the wait looks for Ctrl-C.
const POLL_MS: u64 = 50;

/// The shell's Job (it starts only `scriptrun`) and its `Sched` handle, found
/// once; `None` before the first script, or when the shell has neither.
pub struct Pool {
    job: Cell<Option<(Handle, Handle)>>,
    tried: Cell<bool>,
}

impl Pool {
    pub const fn new() -> Self {
        Self {
            job: Cell::new(None),
            tried: Cell::new(false),
        }
    }

    /// The Job and Sched handles, keeping the pool on the first use.
    fn handles(&self) -> Option<(Handle, Handle)> {
        if !self.tried.replace(true) {
            let job = startup(|info| {
                info.kind() == Some(Kind::Job) && info.rights().contains(Rights::CREATE)
            });
            let sched = startup(|info| info.kind() == Some(Kind::Sched));
            if let (Some(job), Some(sched)) = (job, sched)
                && sys::job_pool(job, POOL).is_ok()
            {
                self.job.set(Some((job, sched)));
            }
        }
        self.job.get()
    }

    /// Runs `request` in a child: its answer, or why there is none. `None` when the
    /// shell has no pool, so it runs the script itself.
    pub fn run(&self, request: &str, limit_ms: u64) -> Option<Result<String, String>> {
        let (job, sched) = self.handles()?;
        let shm = sys::shm_create(PAGES).ok()?;
        let answer = claim(job, sched, shm, request, limit_ms);
        let _ = sys::handle_close(shm);
        Some(answer)
    }
}

fn startup(wanted: impl Fn(&HandleInfo) -> bool) -> Option<Handle> {
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| wanted(info))
        .map(|info| Handle(info.handle))
}

fn claim(
    job: Handle,
    sched: Handle,
    shm: Handle,
    request: &str,
    limit_ms: u64,
) -> Result<String, String> {
    let failed =
        |what: &str, error: kabi::Error| format!("the script's process: {what} ({error:?})");
    write(shm, request.as_bytes()).map_err(|error| failed("cannot write the request", error))?;
    // Only what the run tier needs, and only for this claim: JOB_DONE or the
    // child's end empties its table.
    let given_sched = sys::handle_dup(sched, Rights::READ | Rights::SET_POLICY | Rights::TRANSFER)
        .map_err(|error| failed("cannot narrow the Sched handle", error))?;
    let given_shm = sys::handle_dup(
        shm,
        Rights::MAP | Rights::READ | Rights::WRITE | Rights::TRANSFER,
    )
    .map_err(|error| failed("cannot give the request", error))?;
    let pid = sys::job_claim(job, "mode=script", &[given_sched, given_shm])
        .map_err(|error| failed("no child to run it", error))?;
    let exit = wait(job, pid, limit_ms)?;
    match ExitReason::from_raw(exit.reason) {
        Some(ExitReason::Exited) if exit.detail == 0 => {
            read(shm).map_err(|error| failed("cannot read the answer", error))
        }
        Some(ExitReason::Exited) => Err(format!("the script's process failed ({})", exit.detail)),
        Some(reason) => Err(format!("the script's process {}", reason.name())),
        None => Err(String::from(
            "the script's process ended for no known reason",
        )),
    }
}

/// Waits for child `pid`, ending it on Ctrl-C or once `limit_ms` passes.
fn wait(job: Handle, pid: u64, limit_ms: u64) -> Result<ExitRecord, String> {
    let deadline = sys::time_ns().saturating_add(limit_ms.saturating_mul(1_000_000));
    let mut why = None;
    loop {
        match sys::job_wait(job, POLL_MS) {
            Ok(Some(exit)) if exit.pid == pid => {
                return match why {
                    Some(why) => Err(why),
                    None => Ok(exit),
                };
            }
            Ok(_) => {}
            Err(error) => return Err(format!("the script's process: cannot wait ({error:?})")),
        }
        if why.is_some() {
            continue;
        }
        let stop = match (sys::interrupted(), sys::time_ns() >= deadline) {
            (true, _) => Some("stopped: Ctrl-C"),
            (false, true) => Some("stopped: the script ran past its time limit"),
            (false, false) => None,
        };
        if let Some(stop) = stop {
            why = Some(String::from(stop));
            let _ = sys::job_kill(job, pid);
        }
    }
}

/// The request's length, then its bytes, at the start of the `Shm`.
fn write(shm: Handle, bytes: &[u8]) -> Result<(), kabi::Error> {
    let address = sys::shm_map(shm, true)?;
    let page = region(address);
    let fits = bytes.len() + 8 <= page.len();
    if fits {
        page[..8].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        page[8..8 + bytes.len()].copy_from_slice(bytes);
    }
    sys::shm_unmap(address)?;
    match fits {
        true => Ok(()),
        false => Err(kabi::Error::Invalid),
    }
}

fn read(shm: Handle) -> Result<String, kabi::Error> {
    let address = sys::shm_map(shm, false)?;
    let page = region(address);
    let mut len = [0u8; 8];
    len.copy_from_slice(&page[..8]);
    let len = usize::try_from(u64::from_le_bytes(len)).unwrap_or(usize::MAX);
    let text = page
        .get(8..8usize.saturating_add(len))
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    sys::shm_unmap(address)?;
    text.ok_or(kabi::Error::Invalid)
}

fn region(address: u64) -> &'static mut [u8] {
    // SAFETY: SHM_MAP mapped PAGES pages at `address` for this process, and only
    // this thread touches them until the unmap.
    unsafe { core::slice::from_raw_parts_mut(address as *mut u8, PAGES as usize * 4096) }
}
