//! Raw system calls. Each wrapper returns the kernel's result decoded by
//! `kabi::Error::from_raw`.

use kabi::{CallRequest, Error, ExitRecord, HttpRequest, Metrics, SpawnRequest, call};

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
    Ok((got == 1).then_some(record))
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

/// Sends `request` to the service `name` and returns the length of its reply,
/// which is copied into `reply` (cut to fit).
pub fn service_call(
    name: &str,
    request: &[u8],
    reply: &mut [u8],
    timeout_ms: u64,
) -> Result<usize, Error> {
    let argument = CallRequest {
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        reply_ptr: reply.as_mut_ptr() as u64,
        reply_cap: reply.len() as u64,
        timeout_ms,
    };
    call(
        call::CALL,
        [
            name.as_ptr() as u64,
            name.len() as u64,
            &argument as *const CallRequest as u64,
            0,
            0,
            0,
        ],
    )
    .map(|len| len as usize)
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
