//! The one background build (M9d). BUILD_START hands a `POST /build` request to
//! a kernel thread, which sends it to the build service and waits for the reply;
//! BUILD_POLL collects it. The job belongs to whoever holds PUBLISH, not to a
//! process, so it outlives an edit service that dies, and the next one collects
//! it. The reply is kept for COMP_PUT before the job is marked done, so the upload
//! that follows is checked against it exactly as before.

use crate::{build_service, log, net, threads};
use alloc::vec::Vec;
use kabi::Error;
use spin::Mutex;

/// What the thread sends.
struct Work {
    request: Vec<u8>,
    timeout_ms: u64,
    /// Most bytes of the reply kept.
    cap: usize,
}

enum Phase {
    /// The thread takes the work out when it starts.
    Running(Option<Work>),
    /// The reply, or why there is none, until it is collected.
    Done(Result<Vec<u8>, Error>),
}

static JOB: Mutex<Option<Phase>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Option<Phase>) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut JOB.lock());
    threads::preempt_enable();
    result
}

/// Starts the job: Invalid unless `request` is a build, NotFound without a build
/// service, Busy while a job runs or its result is not collected.
pub fn start(request: Vec<u8>, timeout_ms: u64, cap: usize) -> Result<(), Error> {
    if !editcore::wire::is_build_request(&request) {
        return Err(Error::Invalid);
    }
    if build_service::target().is_none() {
        return Err(Error::NotFound);
    }
    let work = Work {
        request,
        timeout_ms,
        cap,
    };
    // The request is freed after the lock, whichever way this goes.
    let refused = with(|job| match job {
        Some(_) => Some(work),
        None => {
            *job = Some(Phase::Running(Some(work)));
            None
        }
    });
    if refused.is_some() {
        return Err(Error::Busy);
    }
    if let Err(why) = threads::spawn("build-job", run, 0) {
        log!("build: cannot start the build thread: {why}");
        drop(with(Option::take));
        return Err(Error::NoMemory);
    }
    build_service::count_build();
    Ok(())
}

fn run(_: usize) {
    let Some(work) = with(|job| match job {
        Some(Phase::Running(work)) => work.take(),
        _ => None,
    }) else {
        return;
    };
    let target = build_service::target().ok_or(Error::NotFound);
    let result = target.and_then(|target| {
        net::exchange(target, &work.request, work.timeout_ms, work.cap, || false).map_err(|error| {
            log!("build: the build request failed: {error}");
            net::error_code(error)
        })
    });
    drop(work);
    // Kept first: once the service sees the reply, its upload must find it. A
    // connection closed with nothing sent back is no reply, so a look never takes one.
    let result = result.and_then(|reply| {
        if reply.is_empty() {
            return Err(Error::Network);
        }
        let mut kept = Vec::new();
        kept.try_reserve_exact(reply.len())
            .map_err(|_| Error::NoMemory)?;
        kept.extend_from_slice(&reply);
        build_service::keep_build_reply(kept);
        Ok(reply)
    });
    with(|job| *job = Some(Phase::Done(result)));
}
/// Collects the result: NotFound with no job, Busy while it runs. An exchange
/// that failed returns its error once and ends the job. A reply ends it only when
/// it fits `cap`, and comes back as `(len, Some(reply))`; otherwise `(len, None)`
/// and it stays, so a `cap` of 0 only looks.
pub fn take(cap: usize) -> Result<(usize, Option<Vec<u8>>), Error> {
    with(|job| match job.take() {
        None => Err(Error::NotFound),
        Some(Phase::Done(Err(error))) => Err(error),
        Some(Phase::Done(Ok(reply))) if reply.len() <= cap => Ok((reply.len(), Some(reply))),
        Some(phase) => {
            let result = match &phase {
                Phase::Done(Ok(reply)) => Ok((reply.len(), None)),
                _ => Err(Error::Busy),
            };
            *job = Some(phase);
            result
        }
    })
}

/// Puts back a reply [`take`] returned that could not be copied out.
pub fn put_back(reply: Vec<u8>) {
    with(|job| {
        if job.is_none() {
            *job = Some(Phase::Done(Ok(reply)));
        }
    });
}
