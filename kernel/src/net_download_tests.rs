//! `test=net-download [again]` (P5 task 6's case, the roadmap's P5 exit: "a
//! cut connection resumes, and a bad certificate or bad hash is refused").
//! The plan starts `fs` on a fresh `kai-data`, then `dlcheck` with FILES, NET
//! and a `NetDest` for the HTTPS mock's five hosts, which downloads through
//! `kainet` over TLS into kaifs:
//! - boot 1: a target of 32 chunks, cut at 40% by the mock and resumed, stops
//!   inside chunk 16 as a reboot would, its mark in kaifs; a flipped chunk and
//!   a wrong whole-file hash are refused, and so are an expired, a wrong-name
//!   and an unknown-CA certificate, before a byte is stored;
//! - boot 2 (`again`, the same disk): it resumes at the mark, is cut at 80%,
//!   resumes again, and the whole target checks out before it is moved in.
//!
//! The job list `dlcheck` runs is pinned by its SHA-256 in
//! `netcore::fixture`, as signed metadata pins a target. Under TCG
//! (`accel=tcg`, which xtask adds) its target is the same 32 chunks at
//! 64 KiB, not 2 MiB. The kernel waits for `dlcheck`'s mark in `/home/user`.

use crate::files_tests::ask;
use crate::{ipc, log, threads};
use alloc::format;
use alloc::string::String;
use fsproto::{How, Reply, Request};
use netcore::fixture;

/// How long `dlcheck` may take, inside the case's 300 s: a 64 MiB download
/// under KVM, or TCG's 32 small chunks and their handshakes.
const WAIT_MS: u64 = 270_000;
const POLL_MS: u64 = 100;

/// `fs`, then `dlcheck` for `boot`, on the job list for the accelerator.
pub fn plan(again: bool, small: bool) -> String {
    let (jobs, pin) = match small {
        true => (
            fixture::DOWNLOAD_JOBS_SMALL,
            fixture::DOWNLOAD_JOBS_SMALL_SHA256,
        ),
        false => (fixture::DOWNLOAD_JOBS, fixture::DOWNLOAD_JOBS_SHA256),
    };
    format!(
        "{fs}\ndlcheck caps=files+net netdest=proto:tcp,ports:443,hosts:{releases}+{assets}+{repo}+{models}+{off} pages=4096 stack_pages=64 burst_ms=60000 boot={boot} jobs=https://{releases}{release}{jobs} pin={pin}",
        fs = kabi::plan::FS,
        releases = fixture::MOCK_RELEASES,
        assets = fixture::MOCK_ASSETS,
        repo = fixture::MOCK_REPO,
        models = fixture::MOCK_MODELS,
        off = fixture::MOCK_OFF,
        boot = if again { 2 } else { 1 },
        release = fixture::DOWNLOAD_RELEASE,
    )
}

/// Waits for `dlcheck`'s mark for this boot.
pub fn check(again: bool) -> Result<(), &'static str> {
    let boot = if again { 2 } else { 1 };
    let (ok, fail) = (
        format!("net-download-{boot}.ok"),
        format!("net-download-{boot}.fail"),
    );
    let mut waited = 0;
    while !exists(&ok) {
        if exists(&fail) {
            return Err("net-download: dlcheck failed; its lines above say why");
        }
        if waited >= WAIT_MS {
            return Err("net-download: dlcheck did not finish in time");
        }
        threads::sleep_ms(POLL_MS);
        waited += POLL_MS;
    }
    match again {
        false => log!(
            "net-download: boot 1: dlcheck's download stopped inside chunk 16 with its mark in kaifs, and the bad certificates and hashes were refused"
        ),
        true => log!(
            "net-download: boot 2: dlcheck resumed after the reboot and its target checked out whole"
        ),
    }
    Ok(())
}

/// Whether `/home/user/<name>` exists.
fn exists(name: &str) -> bool {
    ipc::server_of("fs").is_some()
        && matches!(
            ask(
                kabi::fs::HOME_USER,
                Request::Open {
                    path: name.into(),
                    rights: 0,
                    how: How::Existing,
                },
            ),
            Ok(Reply::Opened(..))
        )
}
