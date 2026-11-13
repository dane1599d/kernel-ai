//! Test program for `net-download` (P5 task 6's boot case, the roadmap's P5
//! exit: "a cut connection resumes, and a bad certificate or bad hash is
//! refused"). The case's plan starts it with FILES (its `/home/user`), NET, and
//! a `NetDest` for the HTTPS mock's five hosts. It runs `kainet::download`
//! over `user::tls` into kaifs ([`store`]), as `pkg`, `updated` and Models do.
//!
//! **What it trusts.** `jobs=URL pin=HEX`: a job list whose SHA-256 the case
//! pins, as signed metadata pins a target. Each line of it names a job, its
//! `kai-chunks 1` list with that list's SHA-256, and its target:
//!
//! ```text
//! kai-dl-jobs 1
//! <name> <list URL> <list SHA-256> <target URL>
//! ```
//!
//! **Boot 1** (`boot=1`):
//! - `blob` downloads in 32 chunks; the mock cuts it at 40%, and the resume
//!   fetches again only the chunk in flight. Inside chunk 16 it stops, as a
//!   reboot mid-chunk would, with chunks 0-15 and the mark in kaifs.
//! - `flipped`: a byte of chunk 2 is flipped, so chunk 2 is refused by name
//!   and none of it is written.
//! - `whole`: every chunk matches, the whole target's hash does not; it is
//!   refused and never moved in.
//! - `expired`, `wrong-name`, `unknown-ca`: each target's host has that
//!   certificate, refused before a byte is stored.
//!
//! **Boot 2** (`boot=2`, the same disk): `blob` resumes at the mark, re-checks
//! chunks 0-15 from kaifs, is cut at 80% and resumes again, and its whole hash
//! checks out before it is moved in.
//!
//! It prints a line for each, then leaves `net-download-<boot>.ok` (or
//! `.fail`) in its home for the kernel, and idles: the supervisor would
//! restart a service that exited.

#![no_std]
#![no_main]

extern crate alloc;

mod split;

use user::kaifs as store;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use fsproto::How;
use kabi::handle::{Handle, HandleInfo, Kind, Rights};
use kainet::chunks::Chunks;
use kainet::download::{Answer, DownloadError, Report, download};
use kainet::http::{Fetch, HttpError, Limits, Method, Range, TlsConnect, Url};
use kainet::tls::TlsError;
use rustls::{CertificateError, ClientConfig};
use sha2::{Digest, Sha256};
use split::{Split, TimedConnect, TimedDial, TimedPartial, add, ms};
use store::Kaifs;
use user::tls::{self, NetDial};
use user::{arg, args, println, sys};

user::entry!(main);

/// Where the targets land, under `/home/user`.
const FOLDER: &str = "downloads";
/// The job list's header.
const JOBS: &str = "kai-dl-jobs 1";
/// The longest job list read.
const MAX_JOBS: u64 = 4096;
/// How long one request may take.
const REQUEST_NS: u64 = 60_000_000_000;
/// More cuts than the case makes is a failure.
const MAX_CUTS: usize = 2;

struct Job {
    name: String,
    list: Url,
    list_sha: [u8; 32],
    target: Url,
}

/// The network: the handle every connection is made on, and TLS.
struct Net {
    dest: Handle,
    config: Arc<ClientConfig>,
}

fn main() -> i32 {
    let args = args();
    let boot = arg(&args, "boot")
        .and_then(|b| b.parse().ok())
        .unwrap_or(1u8);
    let Some(home) = store::home() else {
        println!("dlcheck: FAIL no Dir handle at startup");
        idle();
    };
    let result = run(&args, home, boot);
    if let Err(why) = &result {
        println!("dlcheck: FAIL boot {boot}: {why}");
    }
    let marker = match result {
        Ok(()) => format!("net-download-{boot}.ok"),
        Err(_) => format!("net-download-{boot}.fail"),
    };
    // The lines come before the mark: the kernel ends the case once it sees
    // it, and the machine may stop before a later line.
    match store::open(home, &marker, How::CreateFile, Rights::READ) {
        Ok(handle) => {
            if let Some(handle) = handle {
                let _ = sys::handle_close(handle);
            }
        }
        Err(why) => println!("dlcheck: FAIL the mark {marker}: {why}"),
    }
    idle();
}

fn idle() -> ! {
    loop {
        sys::sleep_ms(60_000);
    }
}

fn run(args: &str, home: Handle, boot: u8) -> Result<(), String> {
    let dest = dest().ok_or("no NetDest handle to fetch on")?;
    let config = tls::config().map_err(|e| format!("TLS: {e:?}"))?;
    let net = Net { dest, config };
    let url = arg(args, "jobs").ok_or("no jobs=")?;
    let pin = arg(args, "pin").and_then(hex32).ok_or("no pin=")?;
    let jobs = jobs(&net, url, &pin)?;
    let dir = store::folder(home, FOLDER)?;
    let job = |name: &str| {
        jobs.iter()
            .find(|job| job.name == name)
            .ok_or_else(|| format!("no job {name} in the list"))
    };
    match boot {
        1 => {
            blob(&net, dir, job("blob")?, true)?;
            flipped(&net, dir, job("flipped")?)?;
            whole(&net, dir, job("whole")?)?;
            for name in ["expired", "wrong-name", "unknown-ca"] {
                certificate(&net, dir, job(name)?)?;
            }
            Ok(())
        }
        2 => blob(&net, dir, job("blob")?, false),
        _ => Err(format!("no boot {boot}")),
    }
}

/// The `NetDest` handle the plan line gave: the one that may resolve (NET's
/// own handle, for the LLM server, may only connect).
fn dest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); 16];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind == Kind::NetDest as u32 && info.rights & Rights::RESOLVE.bits() != 0)
        .map(|info| Handle(info.handle))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

/// A small file whole: a 200's body of at most `max` bytes.
fn get(net: &Net, url: &Url, max: u64) -> Result<Vec<u8>, String> {
    let clock = || sys::time_ns();
    let allow = |_: &str, port: u16| port == 443;
    let deadline_ns = clock().saturating_add(REQUEST_NS);
    let fetch = Fetch {
        limits: Limits::new(max, deadline_ns),
        allow: &allow,
        clock: &clock,
    };
    let mut connect = TlsConnect {
        config: Arc::clone(&net.config),
        dial: NetDial {
            dest: net.dest,
            bulk: false,
            deadline_ns,
        },
    };
    let mut body = Vec::new();
    let got = fetch
        .run(&mut connect, Method::Get, url, None, &mut |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        })
        .map_err(|e| format!("GET {}{}: {e:?}", url.host, url.path))?;
    match got.head.status {
        200 => Ok(body),
        status => Err(format!("GET {}{}: {status}", url.host, url.path)),
    }
}

/// The job list at `url`, if its SHA-256 is `pin`.
fn jobs(net: &Net, url: &str, pin: &[u8; 32]) -> Result<Vec<Job>, String> {
    let url = Url::parse(url).map_err(|e| format!("jobs={url}: {e}"))?;
    let text = get(net, &url, MAX_JOBS)?;
    let sha: [u8; 32] = Sha256::digest(&text).into();
    if &sha != pin {
        return Err(String::from("the job list is not the one the case pinned"));
    }
    let text = core::str::from_utf8(&text).map_err(|_| "the job list is not text")?;
    let mut lines = text.lines();
    if lines.next() != Some(JOBS) {
        return Err(format!("the job list does not start {JOBS:?}"));
    }
    lines
        .map(|line| {
            let words: Vec<&str> = line.split(' ').collect();
            let [name, list, sha, target] = words[..] else {
                return Err(format!("a job line {line:?}"));
            };
            let url = |text: &str| Url::parse(text).map_err(|e| format!("{text}: {e}"));
            Ok(Job {
                name: name.into(),
                list: url(list)?,
                list_sha: hex32(sha).ok_or_else(|| format!("{name}'s list hash"))?,
                target: url(target)?,
            })
        })
        .collect()
}

/// `job`'s chunk list, if it is the one the job list names.
fn chunks(net: &Net, job: &Job) -> Result<Chunks, String> {
    let text = get(net, &job.list, kainet::chunks::MAX_LIST as u64)?;
    let sha: [u8; 32] = Sha256::digest(&text).into();
    if sha != job.list_sha {
        return Err(format!(
            "{}'s chunk list is not the one the job list names",
            job.name
        ));
    }
    Chunks::parse(&text).map_err(|e| format!("{}'s chunk list: {e}", job.name))
}

/// What one boot's runs of a download did.
struct Runs {
    /// Requests for each chunk.
    asked: Vec<u8>,
    cuts: usize,
    /// Stopped inside the chunk `stop` named.
    stopped: bool,
    report: Option<Report>,
    /// Body bytes taken, and the time it took.
    bytes: u64,
    ns: u64,
    /// Where that time went.
    split: Split,
}

/// Downloads `job` into `partial`, resuming after each cut (at most
/// [`MAX_CUTS`]); with `stop`, the request for that chunk ends half-way, as a
/// reboot inside it would.
fn runs(
    net: &Net,
    job: &Job,
    list: &Chunks,
    partial: &mut Kaifs,
    stop: Option<usize>,
) -> Result<Runs, DownloadError> {
    let mut got = Runs {
        asked: alloc::vec![0; list.count()],
        cuts: 0,
        stopped: false,
        report: None,
        bytes: 0,
        ns: 0,
        split: Split::default(),
    };
    let started = sys::time_ns();
    loop {
        match once(net, job, list, partial, stop, &mut got) {
            Ok(report) => {
                got.report = Some(report);
                break;
            }
            Err(_) if got.stopped => break,
            Err(DownloadError::Fetch(HttpError::Truncated)) if got.cuts < MAX_CUTS => {
                got.cuts += 1;
            }
            Err(error) => return Err(error),
        }
    }
    got.ns = sys::time_ns().saturating_sub(started);
    Ok(got)
}

/// One run of `download`.
fn once(
    net: &Net,
    job: &Job,
    list: &Chunks,
    partial: &mut Kaifs,
    stop: Option<usize>,
    got: &mut Runs,
) -> Result<Report, DownloadError> {
    let clock = || sys::time_ns();
    // The handle decides which hosts; the URL must still be https on 443.
    let allow = |_: &str, port: u16| port == 443;
    let split = &got.split;
    let mut timed = TimedPartial {
        inner: partial,
        split,
    };
    download(list, &mut timed, &mut |range: Range, sink| {
        let started = clock();
        let index = (range.first / list.chunk) as usize;
        if let Some(asked) = got.asked.get_mut(index) {
            *asked = asked.saturating_add(1);
        }
        // Each chunk is a request of its own, and no wait of it outlasts its
        // deadline.
        let deadline_ns = started.saturating_add(REQUEST_NS);
        let fetch = Fetch {
            limits: Limits::new(list.chunk, deadline_ns),
            allow: &allow,
            clock: &clock,
        };
        let mut connect = TimedConnect {
            inner: TlsConnect {
                config: Arc::clone(&net.config),
                dial: TimedDial {
                    inner: NetDial {
                        dest: net.dest,
                        bulk: true,
                        deadline_ns,
                    },
                    split,
                },
            },
            split,
        };
        let half = list.chunk / 2;
        let mut taken = 0u64;
        let stopping = stop == Some(index);
        let stopped = &mut got.stopped;
        let bytes = &mut got.bytes;
        // From the first body bytes to the last passed on.
        let mut first = None;
        let mut last = 0u64;
        let fetched = fetch.run(
            &mut connect,
            Method::Get,
            &job.target,
            Some(range),
            &mut |b| {
                let now = clock();
                first.get_or_insert(now);
                last = now;
                if stopping && taken + b.len() as u64 >= half {
                    *stopped = true;
                    return Err(HttpError::Connect(
                        "stopped inside the chunk for the reboot",
                    ));
                }
                taken += b.len() as u64;
                *bytes += b.len() as u64;
                let passed = sink(b);
                last = clock();
                passed
            },
        );
        if let Some(first) = first {
            split
                .body
                .set(split.body.get() + last.saturating_sub(first));
        }
        add(&split.fetch, started);
        Ok(Answer::from(&fetched?))
    })
}

/// Chunks fetched more than once, by index.
fn again(asked: &[u8]) -> Vec<usize> {
    (0..asked.len()).filter(|&i| asked[i] > 1).collect()
}

/// `blob`: boot 1 cuts once and stops inside the middle chunk; boot 2 resumes
/// at the mark, cuts once more, and finishes.
fn blob(net: &Net, dir: Handle, job: &Job, first: bool) -> Result<(), String> {
    let list = chunks(net, job)?;
    let count = list.count();
    let middle = count / 2;
    let marked = store::marked(dir, &job.name).map_err(|e| format!("blob's mark: {e}"))?;
    let mut partial = Kaifs::new(dir, &job.name);
    let want = if first { 0 } else { middle };
    if marked != want {
        return Err(format!(
            "blob's mark says {marked} at the start, not {want}"
        ));
    }
    let stop = first.then_some(middle);
    let got = runs(net, job, &list, &mut partial, stop);
    let why = partial.why.take();
    partial.close();
    let got = got.map_err(|e| format!("blob: {e:?} {}", why.unwrap_or_default()))?;
    let twice = again(&got.asked);
    let asked_most = got.asked.iter().copied().max().unwrap_or(0);
    if got.cuts != 1 || twice.len() != 1 || asked_most > 2 {
        return Err(format!(
            "blob: {} cuts, chunks {twice:?} fetched again, {asked_most} at most",
            got.cuts
        ));
    }
    let cut = twice[0];
    // MB/s in hundredths: bytes a nanosecond is GB/s.
    let rate = got.bytes * 100_000 / got.ns.max(1);
    println!(
        "dlcheck: blob: {} bytes fetched in {} ms with the checks and kaifs, {}.{:02} MB/s (chunks of {} bytes)",
        got.bytes,
        got.ns / 1_000_000,
        rate / 100,
        rate % 100,
        list.chunk
    );
    print_split(&got, list.chunk);
    if first {
        let mark = store::marked(dir, &job.name)?;
        if !got.stopped || mark != middle || got.asked[middle] != 1 {
            return Err(format!(
                "blob: stopped {}, the mark says {mark}, not {middle}",
                got.stopped
            ));
        }
        // Each line fits one console piece (`[dlcheck] ` and 150 characters).
        println!(
            "dlcheck: boot 1: {middle} of {count} chunks checked and written, and the mark in kaifs says {middle}"
        );
        println!(
            "dlcheck: boot 1: the cut at chunk {cut} fetched again only that chunk; stopped inside chunk {middle} for the reboot"
        );
        return Ok(());
    }
    let size = store::size(dir, &job.name)?;
    if got.report.is_none()
        || size != Some(list.size)
        || got.asked[..middle].iter().any(|&n| n != 0)
    {
        return Err(format!(
            "blob: not finished, or fetched below the mark ({size:?})"
        ));
    }
    println!(
        "dlcheck: boot 2: resumed at the mark, chunk {middle} of {count}, re-checking chunks 0 to {} from kaifs without fetching them",
        middle - 1
    );
    println!(
        "dlcheck: boot 2: the cut at chunk {cut} fetched again only that chunk; the whole target checked out and was moved in"
    );
    Ok(())
}

/// Where `got`'s time went ([`split`]), a line each for the network, kaifs,
/// and the rest. Each fits one console piece.
fn print_split(got: &Runs, chunk: u64) {
    let s = &got.split;
    // The bodies' own rate, in hundredths of a MB/s.
    let body = got.bytes * 100_000 / s.body.get().max(1);
    println!(
        "dlcheck: split: fetches {} ms: {} connections, dns {}, tcp {}, tls {}; heads {}; bodies {} at {}.{:02} MB/s",
        ms(s.fetch.get()),
        s.connections.get(),
        ms(s.dns.get()),
        ms(s.tcp.get()),
        ms(s.tls()),
        ms(s.head()),
        ms(s.body.get()),
        body / 100,
        body % 100
    );
    let rest = got.ns.saturating_sub(s.fetch.get() + s.kaifs());
    println!(
        "dlcheck: split: kaifs {} ms: {} writes {}, {} marks {}, re-reads {}, move-in {}; the rest {} ms",
        ms(s.kaifs()),
        s.writes.get(),
        ms(s.write.get()),
        s.marks.get(),
        ms(s.mark.get()),
        ms(s.read.get()),
        ms(s.finish.get()),
        ms(rest)
    );
    println!(
        "dlcheck: split: the rest is hashes and copies; download hashes each chunk twice, and one SHA-256 of a chunk takes {} us",
        split::sha256_us(usize::try_from(chunk).unwrap_or(usize::MAX))
    );
}

/// `flipped`: refused at its flipped chunk, which is never written.
fn flipped(net: &Net, dir: Handle, job: &Job) -> Result<(), String> {
    let list = chunks(net, job)?;
    let mut partial = Kaifs::new(dir, &job.name);
    let got = runs(net, job, &list, &mut partial, None);
    partial.close();
    let Err(DownloadError::Chunk(bad)) = got else {
        return Err(String::from("flipped: not refused by its chunk"));
    };
    let mark = store::marked(dir, &job.name)?;
    let size = store::size(dir, &format!("{}.partial", job.name))?;
    if mark != bad || size != Some(bad as u64 * list.chunk) {
        return Err(format!(
            "flipped: chunk {bad} refused, mark {mark}, partial {size:?}"
        ));
    }
    println!(
        "dlcheck: flipped: a flipped byte was refused by its chunk, {bad}, and none of that chunk was written: the mark says {mark}"
    );
    Ok(())
}

/// `whole`: every chunk matches, the whole hash does not.
fn whole(net: &Net, dir: Handle, job: &Job) -> Result<(), String> {
    let list = chunks(net, job)?;
    let mut partial = Kaifs::new(dir, &job.name);
    let got = runs(net, job, &list, &mut partial, None);
    partial.close();
    if !matches!(got, Err(DownloadError::Whole)) {
        return Err(String::from(
            "whole: not refused by the whole target's hash",
        ));
    }
    if store::size(dir, &job.name)?.is_some() {
        return Err(String::from("whole: refused, yet moved in"));
    }
    println!(
        "dlcheck: whole: every chunk matched and the whole-file hash did not, so it was refused and not moved in"
    );
    Ok(())
}

/// What a refused certificate was, by the job's name.
fn certificate_kind(error: &DownloadError) -> Option<&'static str> {
    let DownloadError::Fetch(HttpError::Tls(TlsError::Tls(rustls::Error::InvalidCertificate(
        kind,
    )))) = error
    else {
        return None;
    };
    Some(match kind {
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => "expired",
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            "wrong-name"
        }
        CertificateError::UnknownIssuer => "unknown-ca",
        _ => "another certificate error",
    })
}

/// `expired`, `wrong-name`, `unknown-ca`: refused in the handshake, before
/// anything is stored.
fn certificate(net: &Net, dir: Handle, job: &Job) -> Result<(), String> {
    let list = chunks(net, job)?;
    let mut partial = Kaifs::new(dir, &job.name);
    let got = runs(net, job, &list, &mut partial, None);
    let partial_name = partial.partial_name();
    let mark_name = partial.mark_name();
    partial.close();
    let kind = got
        .as_ref()
        .err()
        .and_then(certificate_kind)
        .ok_or_else(|| format!("{}: not refused by its certificate", job.name))?;
    if kind != job.name {
        return Err(format!("{}: refused as {kind}", job.name));
    }
    let stored = [partial_name, mark_name, job.name.clone()]
        .iter()
        .map(|name| store::size(dir, name))
        .collect::<Result<Vec<_>, _>>()?;
    if stored.iter().any(Option::is_some) {
        return Err(format!("{}: refused, yet something was stored", job.name));
    }
    println!(
        "dlcheck: {}: {} was refused in the handshake, before a byte was stored",
        job.name,
        match kind {
            "expired" => "an expired certificate",
            "wrong-name" => "a certificate for another name",
            _ => "a certificate from an unknown CA",
        }
    );
    Ok(())
}
