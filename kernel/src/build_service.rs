//! The kernel's side of the build service (M9b). `build=IP:PORT` and
//! `build_token=HEX` on the kernel command line name the one place the BUILD call
//! sends to, the same way `llm=` names the LLM server. The edit service is planned
//! only when they are set, and a kernel thread hands each commit and rollback the
//! ledger accepts to the edit service, which records it in git, until git
//! acknowledges it.

use crate::process::components;
use crate::{ipc, log, threads};
use alloc::format;
use alloc::string::{String, ToString};
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU64, Ordering};
use editcore::wire::{self, Ipc, SyncRequest};
use spin::Once;

/// The token is 32 hex digits.
const TOKEN_LEN: usize = 32;
/// The service has 30 s for a sync; this adds time to reach it.
const SYNC_CALL_MS: u64 = 35_000;
const SYNC_POLL_MS: u64 = 250;
const SYNC_BACKOFF_MAX_MS: u64 = 5_000;

struct Config {
    server: (Ipv4Addr, u16),
    token: String,
}

static CONFIG: Once<Config> = Once::new();
/// BUILD exchanges that reached the network, for the tests.
static EXCHANGES: AtomicU64 = AtomicU64::new(0);

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

pub fn count_exchange() {
    EXCHANGES.fetch_add(1, Ordering::SeqCst);
}

/// BUILD exchanges so far.
pub fn exchanges() -> u64 {
    EXCHANGES.load(Ordering::SeqCst)
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

fn sync_thread(_: usize) {
    let mut backoff = SYNC_POLL_MS;
    let mut last_error = String::new();
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
                if error != last_error {
                    let clean = llmproto::text::printable(&error, 160, 1);
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
