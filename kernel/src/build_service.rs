//! The kernel's side of the build service (M9b). `build=IP:PORT` and
//! `build_token=HEX` on the kernel command line name the one place the BUILD call
//! sends to, the same way `llm=` names the LLM server. The edit service is planned
//! only when they are set, and a kernel thread hands each commit and rollback the
//! ledger accepts to the edit service, which records it in git, until git
//! acknowledges it.

use crate::process::components;
use crate::{hpet, ipc, log, threads};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU64, Ordering};
use editcore::wire::{self, Ipc, SyncRequest};
use spin::{Mutex, Once};

/// The token is 32 hex digits.
const TOKEN_LEN: usize = 32;
/// The service has 30 s for a sync; this adds time to reach it.
const SYNC_CALL_MS: u64 = 35_000;
const SYNC_POLL_MS: u64 = 250;
const SYNC_BACKOFF_MAX_MS: u64 = 5_000;
/// Refusals of one job, over at least a minute, before it is given up.
const SYNC_REFUSALS: u32 = 5;
const SYNC_GIVE_UP_NS: u64 = 60_000_000_000;

struct Config {
    server: (Ipv4Addr, u16),
    token: String,
}

static CONFIG: Once<Config> = Once::new();
/// Builds (`POST /build` through BUILD) sent so far, for the tests.
static BUILDS: AtomicU64 = AtomicU64::new(0);
/// The build service's last reply to a build, which the next upload must match.
static LAST_BUILD: Mutex<Option<Vec<u8>>> = Mutex::new(None);

fn valid_token(token: &str) -> bool {
    token.len() == TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Reads `build=IP:PORT` and `build_token=HEX`; both are needed.
pub fn configure(cmdline: &str) {
    let mut server = None;
    let mut token = None;
    for arg in cmdline.split_whitespace() {
        if let Some(value) = arg.strip_prefix("build=") {
            server = crate::net::parse_endpoint(value);
            if server.is_none() {
                log!("build: ignoring build={value}: expected IPv4:PORT");
            }
        } else if let Some(value) = arg.strip_prefix("build_token=") {
            if valid_token(value) {
                token = Some(value);
            } else {
                log!("build: ignoring a build_token that is not {TOKEN_LEN} hex digits");
            }
        }
    }
    match (server, token) {
        (Some(server), Some(token)) => {
            CONFIG.call_once(|| Config {
                server,
                token: token.to_string(),
            });
        }
        (Some(_), None) => log!("build: build= needs build_token= as well; no build service"),
        _ => {}
    }
}

/// Where BUILD sends, if the kernel has a build service.
pub fn target() -> Option<(Ipv4Addr, u16)> {
    CONFIG.get().map(|config| config.server)
}

/// The edit service's plan line, when there is a build service to edit with.
pub fn plan_line() -> Option<String> {
    let config = CONFIG.get()?;
    let (ip, port) = config.server;
    Some(format!(
        "{} host={ip}:{port} token={}",
        kabi::plan::EDIT_BASE,
        config.token
    ))
}

pub fn count_build() {
    BUILDS.fetch_add(1, Ordering::SeqCst);
}

/// Builds sent so far.
pub fn builds() -> u64 {
    BUILDS.load(Ordering::SeqCst)
}

fn with_last<R>(f: impl FnOnce(&mut Option<Vec<u8>>) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut LAST_BUILD.lock());
    threads::preempt_enable();
    result
}

/// Keeps the reply to the latest build, dropping the one before.
pub fn keep_build_reply(reply: Vec<u8>) {
    drop(with_last(|last| last.replace(reply)));
}

/// The reply to the latest build, once: each build can back one upload.
pub fn take_build_reply() -> Option<Vec<u8>> {
    with_last(Option::take)
}

/// Turns on the ledger outboxes and starts the thread that empties them.
pub fn start_sync() -> Result<(), &'static str> {
    if CONFIG.get().is_none() {
        return Ok(());
    }
    components::enable_sync();
    threads::spawn("git-sync", sync_thread, 0)?;
    Ok(())
}

/// Whether a failed sync will fail the same way again: the build service called the
/// request bad (400, 403, 413), or the answer is not an acknowledgement. Transport
/// errors, timeouts and git being busy (503) are worth retrying as long as it takes.
fn permanent(error: &str) -> bool {
    ["400", "403", "413"]
        .iter()
        .any(|status| error.starts_with(&format!("err build service: HTTP {status}")))
        || error.starts_with("ok ")
}

fn sync_thread(_: usize) {
    let mut backoff = SYNC_POLL_MS;
    let mut last_error = String::new();
    // The job that failed for good, how often, and since when.
    let mut refused: (u64, u32, u64) = (0, 0, 0);
    loop {
        let Some((name, job)) = components::next_sync() else {
            threads::sleep_ms(SYNC_POLL_MS);
            continue;
        };
        let request = SyncRequest {
            component: &name,
            label: &job.label,
            outcome: job.outcome,
            generation: job.generation,
            source: &job.source,
            commit: &job.commit,
        };
        let message = wire::encode_ipc(&Ipc::Sync {
            seq: job.seq,
            job: request,
        });
        let result = ipc::call("edit", &message, SYNC_CALL_MS)
            .map_err(|error| format!("{error:?}"))
            .and_then(|reply| wire::parse_ack(&reply, job.seq));
        match result {
            Ok(sha) => {
                components::ack_sync(&name, job.seq, &sha);
                backoff = SYNC_POLL_MS;
                last_error.clear();
            }
            Err(error) => {
                let clean = llmproto::text::printable(&error, 160, 1);
                if permanent(&error) {
                    let now = hpet::nanos();
                    refused = match refused {
                        (seq, n, since) if seq == job.seq => (seq, n + 1, since),
                        _ => (job.seq, 1, now),
                    };
                    let long = now.saturating_sub(refused.2) >= SYNC_GIVE_UP_NS;
                    if refused.1 >= SYNC_REFUSALS && long {
                        log!(
                            "build: git refused {name} {} {SYNC_REFUSALS} times ({clean}); dropped",
                            job.label
                        );
                        components::drop_sync(&name, job.seq);
                        continue;
                    }
                }
                if error != last_error {
                    log!(
                        "build: git sync of {name} {} failed: {clean}; retrying",
                        job.label
                    );
                    last_error = error;
                }
                threads::sleep_ms(backoff);
                backoff = (backoff * 2).min(SYNC_BACKOFF_MAX_MS);
            }
        }
    }
}
