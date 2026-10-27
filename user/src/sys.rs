//! Raw system calls. Each wrapper returns the kernel's result decoded by
//! `kabi::Error::from_raw`.

use core::sync::atomic::AtomicU32;
use kabi::handle::{AbiInfo, CallH, Handle, HandleInfo, RecvInfo, Rights};
use kabi::{Error, ExitRecord, HttpRequest, Metrics, SpawnRequest, call};

/// # Safety
/// The arguments must be valid for call `number` as `kabi` documents it.
#[inline(always)]
unsafe fn syscall(number: u64, a: [u64; 6]) -> u64 {
    let result: u64;
    // SAFETY: the kernel preserves every register except RAX, RCX and R11.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") number => result,
            in("rdi") a[0],
            in("rsi") a[1],
            in("rdx") a[2],
            in("r10") a[3],
            in("r8") a[4],
            in("r9") a[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

fn call(number: u64, a: [u64; 6]) -> Result<u64, Error> {
    // SAFETY: every caller below passes pointers to live buffers of the stated size.
    Error::from_raw(unsafe { syscall(number, a) })
}

pub fn exit(code: i32) -> ! {
    let _ = call(call::EXIT, [code as u32 as u64, 0, 0, 0, 0, 0]);
    unreachable!("exit returned");
}

pub fn write(bytes: &[u8]) -> Result<u64, Error> {
    call(
        call::WRITE,
        [bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0],
    )
}

/// Like [`write`], but for the log only: serial and the terminal's `:log`, not the
/// screen.
pub fn write_log(bytes: &[u8]) -> Result<u64, Error> {
    call(
        call::WRITE,
        [
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            call::WRITE_LOG_ONLY,
            0,
            0,
            0,
        ],
    )
}

pub fn sleep_ms(ms: u64) {
    let _ = call(call::SLEEP, [ms, 0, 0, 0, 0, 0]);
}

pub fn yield_now() {
    let _ = call(call::YIELD, [0; 6]);
}

/// Grows the heap by `bytes` (rounded up to pages) and returns the new end.
pub fn brk(bytes: u64) -> Result<u64, Error> {
    call(call::BRK, [bytes, 0, 0, 0, 0, 0])
}

pub fn time_ns() -> u64 {
    call(call::TIME, [0; 6]).unwrap_or(0)
}

fn read_into(number: u64, buf: &mut [u8], extra: u64) -> Result<usize, Error> {
    call(
        number,
        [buf.as_mut_ptr() as u64, buf.len() as u64, extra, 0, 0, 0],
    )
    .map(|len| len as usize)
}

pub fn args(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::ARGS, buf, 0)
}

pub fn spawn(request: &SpawnRequest) -> Result<u64, Error> {
    call(
        call::SPAWN,
        [request as *const SpawnRequest as u64, 0, 0, 0, 0, 0],
    )
}

/// The next exit record, or `None` after `timeout_ms`.
pub fn wait_child(timeout_ms: u64) -> Result<Option<ExitRecord>, Error> {
    Ok(match wait_event(timeout_ms)? {
        Event::Exit(record) => Some(record),
        Event::Request | Event::Timeout => None,
    })
}

/// What [`wait_event`] returned for.
pub enum Event {
    Exit(ExitRecord),
    /// A request waits for the caller's service; `svc_recv` takes it.
    Request,
    Timeout,
}

/// The next exit record, or a request for the caller's own service (for a
/// supervisor that serves), whichever comes first within `timeout_ms`.
pub fn wait_event(timeout_ms: u64) -> Result<Event, Error> {
    let mut record = ExitRecord::empty();
    let got = call(
        call::WAIT_CHILD,
        [
            &mut record as *mut ExitRecord as u64,
            timeout_ms,
            0,
            0,
            0,
            0,
        ],
    )?;
    Ok(match got {
        1 => Event::Exit(record),
        2 => Event::Request,
        _ => Event::Timeout,
    })
}

/// Stops every process named `name` that the kernel started outside the
/// supervisor, and returns how many went (needs SUPERVISE).
pub fn proc_stop(name: &str) -> Result<u64, Error> {
    call(
        call::PROC_STOP,
        [name.as_ptr() as u64, name.len() as u64, 0, 0, 0, 0],
    )
}

/// [`proc_stop`] with the user's yes: the `ONCE` badge the trusted path gave for
/// menu `id`, which showed `process_stop("<name>")`, or badge 0 under a standing
/// rule the caller holds (needs a Job handle with STOP).
pub fn proc_stop_once(name: &str, badge: u64, id: u64) -> Result<u64, Error> {
    call(
        call::PROC_STOP_ONCE,
        [name.as_ptr() as u64, name.len() as u64, badge, id, 0, 0],
    )
}

pub fn kill(pid: u64) -> Result<u64, Error> {
    call(call::KILL, [pid, 0, 0, 0, 0, 0])
}

pub fn state_save(bytes: &[u8]) -> Result<u64, Error> {
    call(
        call::STATE_SAVE,
        [bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0],
    )
}

pub fn state_load(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::STATE_LOAD, buf, 0)
}

pub fn plan(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::PLAN, buf, 0)
}

/// The next request for this service; `caller` is set to the calling pid (0 for
/// the kernel).
pub fn svc_recv(buf: &mut [u8], timeout_ms: u64, caller: &mut u64) -> Result<usize, Error> {
    call(
        call::SVC_RECV,
        [
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            timeout_ms,
            caller as *mut u64 as u64,
            0,
            0,
        ],
    )
    .map(|len| len as usize)
}

pub fn svc_reply(bytes: &[u8]) -> Result<u64, Error> {
    call(
        call::SVC_REPLY,
        [bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0],
    )
}

/// Pins the calling thread to `cpu` and moves it there; `None` unpins it.
pub fn thread_pin(cpu: Option<usize>) -> Result<(), Error> {
    let cpu = cpu.map_or(u64::MAX, |cpu| cpu as u64);
    call(call::THREAD_PIN, [cpu, 0, 0, 0, 0, 0]).map(|_| ())
}

/// Starts a worker thread running `entry(arg)`, pinned to CPU `cpu`, on a stack of
/// `stack_pages` pages; returns its index (see `kabi::thread`). `entry` must never
/// return: a worker ends with [`thread_exit`].
pub fn thread_spawn(
    entry: extern "C" fn(u64) -> !,
    arg: u64,
    cpu: usize,
    stack_pages: u64,
) -> Result<usize, Error> {
    call(
        call::THREAD_SPAWN,
        [entry as usize as u64, arg, cpu as u64, stack_pages, 0, 0],
    )
    .map(|index| index as usize)
}

/// Ends the calling worker thread. Returns only if the kernel refused, with why:
/// the main thread ends the process with [`exit`] instead.
pub fn thread_exit() -> Error {
    match call(call::THREAD_EXIT, [0; 6]) {
        Err(error) => error,
        Ok(_) => Error::Invalid,
    }
}

/// Sleeps while the word at `word` holds `expected`, until a [`futex_wake`] for it,
/// `timeout_ms` (0 = no limit) or a kill: Ok(0) when woken, Ok(1) at once when the
/// word holds something else.
pub fn futex_wait(word: *const AtomicU32, expected: u32, timeout_ms: u64) -> Result<u64, Error> {
    call(
        call::FUTEX_WAIT,
        [word as u64, u64::from(expected), timeout_ms, 0, 0, 0],
    )
}

/// Wakes up to `count` threads of this process waiting on `word`; returns how many.
pub fn futex_wake(word: *const AtomicU32, count: u32) -> Result<u64, Error> {
    call(
        call::FUTEX_WAKE,
        [word as u64, u64::from(count), 0, 0, 0, 0],
    )
}

/// Maps the model read-only at `kabi::layout::MODEL_BASE`; returns its length.
pub fn model_map() -> Result<u64, Error> {
    call(call::MODEL_MAP, [0; 6])
}

/// **Interim** (S10-3): fills `buf` from byte `offset` of KV region `region` of
/// the `kai-cache` partition (MODEL_KV_LOAD); whole sectors, at most
/// `kabi::model::KV_IO_BYTES`.
pub fn model_kv_load(region: u64, offset: u64, buf: &mut [u8]) -> Result<u64, Error> {
    let a = [
        region,
        offset,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
        0,
        0,
    ];
    call(call::MODEL_KV_LOAD, a)
}

/// **Interim** (S10-3): writes `buf` there (MODEL_KV_SAVE), then makes every
/// write so far durable when `flush`.
pub fn model_kv_save(region: u64, offset: u64, buf: &[u8], flush: bool) -> Result<u64, Error> {
    let a = [
        region,
        offset,
        buf.as_ptr() as u64,
        buf.len() as u64,
        u64::from(flush),
        0,
    ];
    call(call::MODEL_KV_SAVE, a)
}

pub fn http(request: &HttpRequest) -> Result<usize, Error> {
    call(
        call::HTTP,
        [request as *const HttpRequest as u64, 0, 0, 0, 0, 0],
    )
    .map(|len| len as usize)
}

pub fn list(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::LIST, buf, 0)
}

/// Sends `request` through the route `route` and returns the length of its reply,
/// which is copied into `reply` (cut to fit).
pub fn call_route(
    route: Handle,
    request: &[u8],
    reply: &mut [u8],
    timeout_ms: u64,
) -> Result<usize, Error> {
    call_h(route, request, reply, timeout_ms, &[], &mut []).map(|(len, _)| len)
}

/// [`call_route`], moving `send` to the server; the handles the reply moved are
/// written to `received` (0 past them). Returns the reply's length and how many
/// handles came back.
pub fn call_h(
    route: Handle,
    request: &[u8],
    reply: &mut [u8],
    timeout_ms: u64,
    send: &[Handle],
    received: &mut [Handle],
) -> Result<(usize, usize), Error> {
    let argument = CallH {
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        reply_ptr: reply.as_mut_ptr() as u64,
        reply_cap: reply.len() as u64,
        timeout_ms,
        handles_ptr: send.as_ptr() as u64,
        handles_len: send.len() as u64,
        handles_out_ptr: received.as_mut_ptr() as u64,
        handles_out_cap: received.len() as u64,
    };
    let len = call(
        call::CALL_H,
        [
            u64::from(route.0),
            &argument as *const CallH as u64,
            0,
            0,
            0,
            0,
        ],
    )?;
    let came = received.iter().filter(|h| **h != Handle::NONE).count();
    Ok((len as usize, came))
}

/// Waits for the next request to this process's service, as [`svc_recv`] does,
/// with who sent it, its route's badge and rights, and the handles it moved.
pub fn recv_h(buf: &mut [u8], timeout_ms: u64, info: &mut RecvInfo) -> Result<usize, Error> {
    call(
        call::RECV_H,
        [
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            timeout_ms,
            info as *mut RecvInfo as u64,
            0,
            0,
        ],
    )
    .map(|len| len as usize)
}

/// Starts the program `job` names (`scriptrun`) as a child, moving `handles` into
/// it; its pid.
pub fn job_spawn(job: Handle, args: &str, handles: &[Handle]) -> Result<u64, Error> {
    let request = kabi::JobSpawn {
        args_ptr: args.as_ptr() as u64,
        args_len: args.len() as u64,
        handles_ptr: handles.as_ptr() as u64,
        handles_len: handles.len() as u64,
    };
    call(
        call::JOB_SPAWN,
        [
            u64::from(job.0),
            &request as *const kabi::JobSpawn as u64,
            0,
            0,
            0,
            0,
        ],
    )
}

/// Keeps up to `n` idle pooled children of the program `job` names; how many.
pub fn job_pool(job: Handle, n: u64) -> Result<u64, Error> {
    call(call::JOB_POOL, [u64::from(job.0), n, 0, 0, 0, 0])
}

/// Gives an idle pooled child `args` and `handles` (or starts one); its pid. Its
/// result comes back through [`job_wait`].
pub fn job_claim(job: Handle, args: &str, handles: &[Handle]) -> Result<u64, Error> {
    let request = kabi::JobSpawn {
        args_ptr: args.as_ptr() as u64,
        args_len: args.len() as u64,
        handles_ptr: handles.as_ptr() as u64,
        handles_len: handles.len() as u64,
    };
    call(
        call::JOB_CLAIM,
        [
            u64::from(job.0),
            &request as *const kabi::JobSpawn as u64,
            0,
            0,
            0,
            0,
        ],
    )
}

/// Ends this process's job child `pid`; JOB_WAIT then reports it Killed.
pub fn job_kill(job: Handle, pid: u64) -> Result<u64, Error> {
    call(call::JOB_KILL, [u64::from(job.0), pid, 0, 0, 0, 0])
}

/// In a pooled child: waits for a claim and copies its arguments; their length.
pub fn job_take(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::JOB_TAKE, buf, 0)
}

/// In a pooled child: reports `code` for the claim and goes back to idle, with
/// every handle closed.
pub fn job_done(code: u64) -> Result<u64, Error> {
    call(call::JOB_DONE, [code, 0, 0, 0, 0, 0])
}

/// The next exit of one of this process's job children, or `None` after
/// `timeout_ms`.
pub fn job_wait(job: Handle, timeout_ms: u64) -> Result<Option<ExitRecord>, Error> {
    let mut record = ExitRecord::empty();
    let got = call(
        call::JOB_WAIT,
        [
            u64::from(job.0),
            &mut record as *mut ExitRecord as u64,
            timeout_ms,
            0,
            0,
            0,
        ],
    )?;
    Ok((got == 1).then_some(record))
}

/// New zeroed shared pages, and an `Shm` handle to them (ADR 0001 D4).
pub fn shm_create(pages: u64) -> Result<Handle, Error> {
    call(call::SHM_CREATE, [pages, 0, 0, 0, 0, 0]).map(|h| Handle(h as u32))
}

/// Maps the `Shm` that `handle` names, read-write if `write`; its address.
pub fn shm_map(handle: Handle, write: bool) -> Result<u64, Error> {
    call(
        call::SHM_MAP,
        [u64::from(handle.0), u64::from(write), 0, 0, 0, 0],
    )
}

/// Takes the screen (`desktop` only): its back buffer's `Shm`, and its shape.
pub fn display_claim(info: &mut kabi::display::DisplayInfo) -> Result<Handle, Error> {
    call(
        call::DISPLAY_CLAIM,
        [
            info as *mut kabi::display::DisplayInfo as u64,
            0,
            0,
            0,
            0,
            0,
        ],
    )
    .map(|h| Handle(h as u32))
}

/// Puts these rectangles of the claimed back buffer on screen.
pub fn display_flush(rects: &[kabi::display::FlushRect]) -> Result<u64, Error> {
    call(
        call::DISPLAY_FLUSH,
        [rects.as_ptr() as u64, rects.len() as u64, 0, 0, 0, 0],
    )
}

/// The 64x64 `0xAARRGGBB` hardware cursor and its hot spot.
pub fn display_cursor(image: &[u32; 64 * 64], hot: (u32, u32)) -> Result<u64, Error> {
    call(
        call::DISPLAY_CURSOR,
        [
            image.as_ptr() as u64,
            u64::from(hot.0),
            u64::from(hot.1),
            0,
            0,
            0,
        ],
    )
}

pub fn display_cursor_move(x: u32, y: u32) -> Result<u64, Error> {
    call(
        call::DISPLAY_CURSOR_MOVE,
        [u64::from(x), u64::from(y), 0, 0, 0, 0],
    )
}

/// Input events from the kernel's queue (`desktop` only), waiting up to
/// `timeout_ms` for the first.
pub fn input_read(out: &mut [kabi::input::InputEvent], timeout_ms: u64) -> Result<usize, Error> {
    let mut raw = [0u8; 64 * kabi::input::InputEvent::SIZE];
    let cap = out.len().min(64);
    let n = call(
        call::INPUT_READ,
        [raw.as_mut_ptr() as u64, cap as u64, timeout_ms, 0, 0, 0],
    )? as usize;
    let mut got = 0;
    for chunk in raw[..n.min(cap) * kabi::input::InputEvent::SIZE]
        .as_chunks::<{ kabi::input::InputEvent::SIZE }>()
        .0
    {
        if let Some(event) = kabi::input::InputEvent::decode(chunk) {
            out[got] = event;
            got += 1;
        }
    }
    Ok(got)
}

/// Removes the `Shm` mapping that starts at `address`.
pub fn shm_unmap(address: u64) -> Result<u64, Error> {
    call(call::SHM_UNMAP, [address, 0, 0, 0, 0, 0])
}

/// Answers the request taken by the last receive, moving `handles` to the caller.
pub fn reply_h(bytes: &[u8], handles: &[Handle]) -> Result<u64, Error> {
    call(
        call::REPLY_H,
        [
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            handles.as_ptr() as u64,
            handles.len() as u64,
            0,
            0,
        ],
    )
}

/// The bound devices into `out`; how many it filled.
pub fn dev_list(out: &mut [kabi::device::DeviceRecord]) -> Result<usize, Error> {
    let bytes = core::mem::size_of_val(out) as u64;
    let len = call(call::DEV_LIST, [out.as_mut_ptr() as u64, bytes, 0, 0, 0, 0])?;
    Ok(len as usize / core::mem::size_of::<kabi::device::DeviceRecord>())
}

pub fn metrics() -> Result<Metrics, Error> {
    let mut metrics = Metrics::empty();
    call(
        call::METRICS,
        [&mut metrics as *mut Metrics as u64, 0, 0, 0, 0, 0],
    )?;
    Ok(metrics)
}

pub fn set_policy(id: u64) -> Result<u64, Error> {
    call(call::POLICY, [id, 0, 0, 0, 0, 0])
}

/// Policy `id` with a `kabi::sched::preset` (it matters only for `classes`).
pub fn set_spec(id: u64, preset: u64) -> Result<u64, Error> {
    call(call::POLICY, [id, preset, 0, 0, 0, 0])
}

/// The scheduler's numbers over `window_ms` into `out`; how many threads.
pub fn sched_stats(out: &mut kabi::sched::SchedStats, window_ms: u64) -> Result<u64, Error> {
    let out = out as *mut kabi::sched::SchedStats as u64;
    call(call::SCHED_STATS, [out, window_ms, 0, 0, 0, 0])
}

/// Starts a kernel-owned policy trial; its id.
pub fn sched_trial(request: &kabi::sched::TrialRequest) -> Result<u64, Error> {
    let request = request as *const kabi::sched::TrialRequest as u64;
    call(call::SCHED_TRIAL, [request, 0, 0, 0, 0, 0])
}

/// Whether the user asked to stop since the last call; reading clears it.
pub fn interrupted() -> bool {
    call(call::INTERRUPTED, [0; 6]).is_ok_and(|flag| flag != 0)
}

/// Copies version `label` of this process's component into `buf` when it fits;
/// its length either way.
pub fn comp_fetch(label: &str, buf: &mut [u8]) -> Result<usize, Error> {
    call(
        call::COMP_FETCH,
        [
            label.as_ptr() as u64,
            label.len() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
            0,
        ],
    )
    .map(|len| len as usize)
}

/// Reports a ledger event for this process's component.
pub fn comp_event(event: &kabi::live::Event) -> Result<(), Error> {
    call(
        call::COMP_EVENT,
        [event as *const kabi::live::Event as u64, 0, 0, 0, 0, 0],
    )
    .map(|_| ())
}

/// The component's ledger, and its checkpoint in `buf` when it fits; the
/// checkpoint's length either way.
pub fn comp_restore(restore: &mut kabi::live::Restore, buf: &mut [u8]) -> Result<usize, Error> {
    call(
        call::COMP_RESTORE,
        [
            restore as *mut kabi::live::Restore as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
            0,
            0,
        ],
    )
    .map(|len| len as usize)
}

/// Uploads a version into the store (needs PUBLISH).
pub fn comp_put(component: &str, artifact: &[u8], base: &str, commit: &str) -> Result<(), Error> {
    let put = kabi::live::Put {
        name_ptr: component.as_ptr() as u64,
        name_len: component.len() as u64,
        artifact_ptr: artifact.as_ptr() as u64,
        artifact_len: artifact.len() as u64,
        base_ptr: base.as_ptr() as u64,
        base_len: base.len() as u64,
        commit_ptr: commit.as_ptr() as u64,
        commit_len: commit.len() as u64,
    };
    call(
        call::COMP_PUT,
        [&put as *const kabi::live::Put as u64, 0, 0, 0, 0, 0],
    )
    .map(|_| ())
}

fn comp_read(read: &kabi::live::Read) -> Result<usize, Error> {
    call(
        call::COMP_READ,
        [read as *const kabi::live::Read as u64, 0, 0, 0, 0, 0],
    )
    .map(|len| len as usize)
}

/// A component's labels and the next upload label (needs PUBLISH).
pub fn comp_read_info(component: &str) -> Result<kabi::live::Info, Error> {
    let mut info = kabi::live::Info::empty();
    comp_read(&kabi::live::Read {
        what: kabi::live::read::INFO,
        name_ptr: component.as_ptr() as u64,
        name_len: component.len() as u64,
        buf_ptr: &mut info as *mut kabi::live::Info as u64,
        buf_cap: core::mem::size_of::<kabi::live::Info>() as u64,
        ..kabi::live::Read::default()
    })?;
    Ok(info)
}

/// Version `label` of a component (the active one when empty) into `buf` when it
/// fits; its length either way (needs PUBLISH).
pub fn comp_read_artifact(component: &str, label: &str, buf: &mut [u8]) -> Result<usize, Error> {
    comp_read(&kabi::live::Read {
        what: kabi::live::read::ARTIFACT,
        name_ptr: component.as_ptr() as u64,
        name_len: component.len() as u64,
        label_ptr: label.as_ptr() as u64,
        label_len: label.len() as u64,
        buf_ptr: buf.as_mut_ptr() as u64,
        buf_cap: buf.len() as u64,
    })
}

/// Has the kernel send `ctl <request>` to a component, which it does only for a
/// request that changes nothing; as much of the reply as fits goes into `reply`,
/// and its whole length comes back (needs PUBLISH).
pub fn comp_control(component: &str, request: &str, reply: &mut [u8]) -> Result<usize, Error> {
    let control = kabi::live::Control {
        name_ptr: component.as_ptr() as u64,
        name_len: component.len() as u64,
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        reply_ptr: reply.as_mut_ptr() as u64,
        reply_cap: reply.len() as u64,
    };
    call(
        call::COMP_CONTROL,
        [&control as *const kabi::live::Control as u64, 0, 0, 0, 0, 0],
    )
    .map(|len| len as usize)
}

/// As [`http`], to the build service the kernel was booted with (needs PUBLISH).
pub fn build(request: &HttpRequest) -> Result<usize, Error> {
    call(
        call::BUILD,
        [request as *const HttpRequest as u64, 0, 0, 0, 0, 0],
    )
    .map(|len| len as usize)
}

/// Starts a build on the build service in the background; [`build_poll`]
/// collects it (needs PUBLISH).
pub fn build_start(request: &HttpRequest) -> Result<(), Error> {
    call(
        call::BUILD_START,
        [request as *const HttpRequest as u64, 0, 0, 0, 0, 0],
    )
    .map(|_| ())
}

/// The background build's reply length, its bytes copied into `buf` (ending the
/// job) only when they fit; an empty `buf` only looks (needs PUBLISH).
pub fn build_poll(buf: &mut [u8]) -> Result<usize, Error> {
    read_into(call::BUILD_POLL, buf, 0)
}

/// The IPv4 addresses of `host` from the kernel's resolver, as many as fit
/// written into `out`; returns how many the answer had (needs NET).
pub fn resolve(host: &str, out: &mut [[u8; 4]]) -> Result<usize, Error> {
    call(
        call::RESOLVE,
        [
            host.as_ptr() as u64,
            host.len() as u64,
            out.as_mut_ptr() as u64,
            out.len() as u64,
            0,
            0,
        ],
    )
    .map(|count| count as usize)
}

/// The kernel's ABI version, highest call number and handle table size.
pub fn abi_info() -> Result<AbiInfo, Error> {
    let mut info = AbiInfo::default();
    call(
        call::ABI_INFO,
        [&mut info as *mut AbiInfo as u64, 0, 0, 0, 0, 0],
    )?;
    Ok(info)
}

/// The handles this process got at spawn and still holds, as many as fit written
/// into `out`; returns how many there are.
pub fn startup(out: &mut [HandleInfo]) -> Result<usize, Error> {
    call(
        call::STARTUP,
        [out.as_mut_ptr() as u64, out.len() as u64, 0, 0, 0, 0],
    )
    .map(|count| count as usize)
}

/// A handle's kind, rights and flags.
pub fn handle_info(handle: Handle) -> Result<HandleInfo, Error> {
    let mut info = HandleInfo::default();
    call(
        call::HANDLE_INFO,
        [
            u64::from(handle.0),
            &mut info as *mut HandleInfo as u64,
            0,
            0,
            0,
            0,
        ],
    )?;
    Ok(info)
}

/// A second handle to `handle`'s object with `rights`, a subset of its own.
pub fn handle_dup(handle: Handle, rights: Rights) -> Result<Handle, Error> {
    call(
        call::HANDLE_DUP,
        [u64::from(handle.0), u64::from(rights.bits()), 0, 0, 0, 0],
    )
    .map(|raw| Handle(raw as u32))
}

/// Swaps `handle` for one with `rights`, a subset of its own; the old one is
/// closed either way.
pub fn handle_replace(handle: Handle, rights: Rights) -> Result<Handle, Error> {
    call(
        call::HANDLE_REPLACE,
        [u64::from(handle.0), u64::from(rights.bits()), 0, 0, 0, 0],
    )
    .map(|raw| Handle(raw as u32))
}

/// Closes `handle` for good.
pub fn handle_close(handle: Handle) -> Result<(), Error> {
    call(call::HANDLE_CLOSE, [u64::from(handle.0), 0, 0, 0, 0, 0]).map(|_| ())
}

/// A new node of the grant tree under the one `parent` names (a `Grant` handle
/// with MINT); `life` 0 is a request, 1 a session, 2 persistent. A `Grant`
/// handle to the new node.
pub fn grant_new(parent: Handle, life: u64, label: &str) -> Result<Handle, Error> {
    call(
        call::GRANT_NEW,
        [
            u64::from(parent.0),
            life,
            label.as_ptr() as u64,
            label.len() as u64,
            0,
            0,
        ],
    )
    .map(|raw| Handle(raw as u32))
}

/// Revokes the node `grant` names and every node below it; how many went.
pub fn grant_revoke(grant: Handle) -> Result<u64, Error> {
    call(call::GRANT_REVOKE, [u64::from(grant.0), 0, 0, 0, 0, 0])
}

/// A second handle to `handle`'s object with `rights`, given under the node
/// `grant` names, so revoking that node revokes it.
pub fn grant_dup(handle: Handle, rights: Rights, grant: Handle) -> Result<Handle, Error> {
    call(
        call::GRANT_DUP,
        [
            u64::from(handle.0),
            u64::from(rights.bits()),
            u64::from(grant.0),
            0,
            0,
            0,
        ],
    )
    .map(|raw| Handle(raw as u32))
}

/// Whether `digest` is the one the trusted path holds for the `ONCE` badge
/// `badge`; a match uses it up.
/// Whether process `pid` holds a standing rule that covers the call `text`, in canonical
/// text (needs SUPERVISE).
pub fn rule_check(pid: u64, text: &str) -> Result<(), Error> {
    call(
        call::RULE_CHECK,
        [pid, text.as_ptr() as u64, text.len() as u64, 0, 0, 0],
    )
    .map(|_| ())
}

/// [`once_check`] for a call made by process `pid`, which must hold the `ONCE`
/// (needs SUPERVISE).
pub fn once_check_for(pid: u64, badge: u64, digest: &[u8; 32]) -> Result<(), Error> {
    call(
        call::ONCE_CHECK_FOR,
        [pid, badge, digest.as_ptr() as u64, 0, 0, 0],
    )
    .map(|_| ())
}

pub fn once_check(badge: u64, digest: &[u8; 32]) -> Result<(), Error> {
    call(
        call::ONCE_CHECK,
        [badge, digest.as_ptr() as u64, 0, 0, 0, 0],
    )
    .map(|_| ())
}

/// The partition a `Partition` handle names (P2 task 3).
pub fn part_info(handle: Handle) -> Result<kabi::part::PartInfo, Error> {
    let mut info = kabi::part::PartInfo::default();
    call(
        call::PART_INFO,
        [
            u64::from(handle.0),
            &mut info as *mut kabi::part::PartInfo as u64,
            0,
            0,
            0,
            0,
        ],
    )?;
    Ok(info)
}

/// Reads `buf.len()` bytes, whole sectors, from sector `sector` of the partition.
pub fn part_read(handle: Handle, sector: u64, buf: &mut [u8]) -> Result<u64, Error> {
    call(
        call::PART_READ,
        [
            u64::from(handle.0),
            sector,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
            0,
        ],
    )
}

/// Writes `bytes`, whole sectors, at sector `sector` of the partition; durable
/// only after [`part_flush`].
pub fn part_write(handle: Handle, sector: u64, bytes: &[u8]) -> Result<u64, Error> {
    call(
        call::PART_WRITE,
        [
            u64::from(handle.0),
            sector,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        ],
    )
}

/// Makes every write to the partition so far durable.
pub fn part_flush(handle: Handle) -> Result<u64, Error> {
    call(call::PART_FLUSH, [u64::from(handle.0), 0, 0, 0, 0, 0])
}

/// Slot `slot`'s partition, through an `Update` handle (ADR 0008 §10).
pub fn slot_info(handle: Handle, slot: u64) -> Result<kabi::update::SlotInfo, Error> {
    let mut info = kabi::update::SlotInfo::default();
    let out = &mut info as *mut kabi::update::SlotInfo as u64;
    call(call::SLOT_INFO, [u64::from(handle.0), slot, out, 0, 0, 0])?;
    Ok(info)
}

/// Reads `buf.len()` bytes, whole sectors, from sector `sector` of slot `slot`.
pub fn slot_read(handle: Handle, slot: u64, sector: u64, buf: &mut [u8]) -> Result<u64, Error> {
    let (ptr, len) = (buf.as_mut_ptr() as u64, buf.len() as u64);
    call(
        call::SLOT_READ,
        [u64::from(handle.0), slot, sector, ptr, len, 0],
    )
}

/// Writes `bytes`, whole sectors, at sector `sector` of slot `slot`; durable
/// only after [`slot_flush`].
pub fn slot_write(handle: Handle, slot: u64, sector: u64, bytes: &[u8]) -> Result<u64, Error> {
    let (ptr, len) = (bytes.as_ptr() as u64, bytes.len() as u64);
    call(
        call::SLOT_WRITE,
        [u64::from(handle.0), slot, sector, ptr, len, 0],
    )
}

/// Makes every write to slot `slot` so far durable.
pub fn slot_flush(handle: Handle, slot: u64) -> Result<u64, Error> {
    call(call::SLOT_FLUSH, [u64::from(handle.0), slot, 0, 0, 0, 0])
}

/// `KaiBootState` as the firmware holds it.
pub fn bootrec_get(handle: Handle) -> Result<[u8; 64], Error> {
    let mut record = [0u8; 64];
    call(
        call::BOOTREC_GET,
        [u64::from(handle.0), record.as_mut_ptr() as u64, 0, 0, 0, 0],
    )?;
    Ok(record)
}

/// Writes `record` and Limine's one-shot (`kabi::update::NONE` deletes it), and
/// with `restart` resets into it.
pub fn bootrec_set(
    handle: Handle,
    record: &[u8; 64],
    one_shot: u64,
    restart: bool,
) -> Result<u64, Error> {
    let args = [
        u64::from(handle.0),
        record.as_ptr() as u64,
        one_shot,
        u64::from(restart),
        0,
        0,
    ];
    call(call::BOOTREC_SET, args)
}

/// A `Dir` or `File` handle on this process's own endpoint, with `badge` and
/// `rights`, for the `fs` service to hand on (P2 task 8).
pub fn endpoint_mint(
    endpoint: Handle,
    kind: kabi::handle::Kind,
    rights: Rights,
    badge: u64,
) -> Result<Handle, Error> {
    call(
        call::ENDPOINT_MINT,
        [
            u64::from(endpoint.0),
            kind as u64,
            u64::from(rights.bits()),
            badge,
            0,
            0,
        ],
    )
    .map(|raw| Handle(raw as u32))
}
