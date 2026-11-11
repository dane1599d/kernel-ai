//! The model download in `updated` (P5 task 9): a target of the signed
//! `models` role fetched from its pinned source into the model store.
//!
//! - **The list first.** The role signs each model's `kai-chunks 1` list as
//!   `<sha256>.chunks`, a target the image's metadata holds but its bundle does
//!   not carry. It is fetched from the repository's targets, consistent-snapshot
//!   named (`targets/<list sha256>.<sha256>.chunks`), held to the signed length
//!   and SHA-256, then to the model (`Job::check_list`), before any range of
//!   the model is asked for.
//! - **A chunk at a time.** `kainet`'s `Download` steps one 2 MiB chunk per
//!   tick of the serve loop, so the shell's and Models' requests are answered
//!   meanwhile. Each chunk is checked against the list before it is written to
//!   `.partial/<sha256>.partial`, with its mark in the same `fs` action
//!   (`user::kaifs`): a cut, a refused chunk or a restart goes on from the last
//!   whole chunk. A failed step leaves the run where it was, so the next try
//!   asks for that chunk again, after a wait.
//! - **Then the copy.** `fs` keeps only sealed blobs, which it hashes as they
//!   are written, so the checked file is copied into the store in one action
//!   (`SealedBegin`, `SealedPush`, `SealedFinish`), a chunk per tick, as an
//!   import is, once the disk has room for it (`modelchan::fetch::disk_need`).
//!   The partial file goes only once the copy is committed.
//!
//! The model's hosts are its source's and that host's storage hosts on 443
//! (`Job::reaches`), on `updated`'s own `NetDest`; the list's is the
//! repository's.

use crate::folders;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use comp_models::{Sha256, hex};
use fsproto::{How, MAX_DATA, Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, Rights};
use kainet::chunks::{Chunks, MAX_LIST};
use kainet::download::{self as dl, Answer, Download, DownloadError, Step};
use kainet::http::{Fetch, HttpError, Limits, Method, TlsConnect, Url};
use kaituf::meta::TargetFile;
use modelchan::fetch::{self, Job, Machine, PARTIAL_IN_STORE};
use rustls::ClientConfig;
use user::kaifs::{self, Kaifs};
use user::tls::{self, NetDial};
use user::{println, sys};

/// How long one chunk's fetch may take, its connection included.
const CHUNK_NS: u64 = 120_000_000_000;
/// How long the list's fetch may take.
const LIST_NS: u64 = 60_000_000_000;
/// Failed tries of one chunk in a row before the download is refused, and
/// the first wait between them, doubled after each, at most `MAX_WAIT_NS`.
const MAX_TRIES: u32 = 8;
const FIRST_WAIT_NS: u64 = 1_000_000_000;
const MAX_WAIT_NS: u64 = 60_000_000_000;
/// Bytes copied into the store per tick: a chunk.
const COPY_STEP: u64 = fetch::MODEL_CHUNK;

/// Where the role's chunk lists are fetched from: the repository's targets,
/// beside its metadata.
pub fn lists(test: bool) -> String {
    let hosts = match test {
        true => kaipkg::online::Hosts::mock(),
        false => kaipkg::online::Hosts::github(),
    };
    let metadata = hosts.metadata;
    let base = metadata.strip_suffix("metadata/").unwrap_or(&metadata);
    format!("{base}targets/")
}

/// The `NetDest` handle among the startup handles.
fn netdest() -> Option<Handle> {
    let mut list = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut list).ok()?.min(MAX_STARTUP);
    list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::NetDest))
        .map(|info| Handle(info.handle))
}

/// `user::kaifs`'s partial file and mark, but a model checked whole is not
/// renamed: it is copied into the store after.
struct ModelPartial(Kaifs);

impl dl::Partial for ModelPartial {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        dl::Partial::checkpoint(&mut self.0)
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        dl::Partial::record(&mut self.0, done)
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        dl::Partial::write(&mut self.0, offset, bytes)
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        dl::Partial::read(&mut self.0, offset, buf)
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        Ok(())
    }
}

impl ModelPartial {
    /// The last error, with what `kaifs` said.
    fn why(&self, error: &DownloadError) -> String {
        match &self.0.why {
            Some(why) => format!("{error:?}: {why}"),
            None => format!("{error:?}"),
        }
    }
}

enum Phase {
    Fetching(Box<Download>),
    /// Copying into the store's sealed blob, in the action it opened: the
    /// bytes copied so far.
    Copying(u64),
}

/// A model download under way.
pub struct Running {
    job: Job,
    dest: Handle,
    config: Arc<ClientConfig>,
    partial: ModelPartial,
    phase: Phase,
    started_ns: u64,
    /// Failed tries of the current chunk in a row, and when to try again.
    tries: u32,
    wait_until_ns: u64,
    report: dl::Report,
}

/// What a finished download says.
pub struct Done {
    pub id: String,
    pub line: String,
}

/// `models download <id>`: the job planned against `machine`, its chunk list
/// fetched and held to the role, and the run started from what `.partial/`
/// holds. The list's target is the role's, from `lists` (`checked.chunks`).
pub fn start(
    store: Handle,
    signed: &[comp_models::Signed],
    lists_signed: &[(Sha256, TargetFile)],
    machine: &Machine<'_>,
    id: &str,
    base: &str,
) -> Result<Running, String> {
    let job = fetch::plan(signed, machine, id).map_err(|why| why.why())?;
    let (_, target) = lists_signed
        .iter()
        .find(|(sha, _)| *sha == job.sha256)
        .ok_or_else(|| format!("the signed models role has no chunk list for {id}"))?;
    let dest = netdest().ok_or("updated has no NetDest handle")?;
    let config = tls::config().map_err(|e| format!("no TLS configuration: {e:?}"))?;
    let bytes = fetch_list(dest, &config, base, target)?;
    let list = Chunks::parse(&bytes).map_err(|why| format!("the chunk list: {why}"))?;
    job.check_list(&list.blob, list.size, list.chunk)
        .map_err(|why| why.why())?;
    let rights = Rights::READ | Rights::WRITE | Rights::ENUMERATE | Rights::CREATE | Rights::DELETE;
    let dir = match kaifs::open(store, PARTIAL_IN_STORE, How::Existing, rights)? {
        Some(dir) => dir,
        None => kaifs::open(store, PARTIAL_IN_STORE, How::Mkdir, rights)?
            .ok_or_else(|| format!("{PARTIAL_IN_STORE} was not made"))?,
    };
    let mut partial = ModelPartial(Kaifs::new(dir, &hex(&job.sha256)));
    let run = Download::start(list, &mut partial).map_err(|e| partial.why(&e))?;
    Ok(Running {
        job,
        dest,
        config,
        partial,
        phase: Phase::Fetching(Box::new(run)),
        started_ns: sys::time_ns(),
        tries: 0,
        wait_until_ns: 0,
        report: dl::Report {
            kept: 0,
            fetched: 0,
        },
    })
}

/// The list `target` from the repository at `base`, held to its signed
/// length and SHA-256.
fn fetch_list(
    dest: Handle,
    config: &Arc<ClientConfig>,
    base: &str,
    target: &TargetFile,
) -> Result<Vec<u8>, String> {
    let sha = kaipkg::models::target_sha256(target)?;
    if target.length == 0 || target.length > MAX_LIST as u64 {
        return Err(format!("the chunk list is {} bytes", target.length));
    }
    let url = format!("{base}{}.{}", hex(&sha), target.path);
    let url = Url::parse(&url).map_err(|why| format!("the chunk list's address: {why}"))?;
    let host = url.host.clone();
    let allow = |h: &str, port: u16| h == host && port == fetch::HTTPS_PORT;
    let clock = || sys::time_ns();
    let deadline_ns = sys::time_ns().saturating_add(LIST_NS);
    let mut connect = TlsConnect {
        config: config.clone(),
        dial: NetDial {
            dest,
            bulk: false,
            deadline_ns,
        },
    };
    let request = Fetch {
        limits: Limits::new(target.length, deadline_ns),
        allow: &allow,
        clock: &clock,
    };
    let mut bytes = Vec::new();
    let got = request
        .run(&mut connect, Method::Get, &url, None, &mut |part| {
            bytes.extend_from_slice(part);
            Ok(())
        })
        .map_err(|e| format!("the chunk list: {e:?}"))?;
    if got.head.status != 200 {
        return Err(format!("the chunk list answered {}", got.head.status));
    }
    if bytes.len() as u64 != target.length || kaipkg::sha256(&bytes) != sha {
        return Err(String::from("the chunk list is not the one the role signs"));
    }
    Ok(bytes)
}

impl Running {
    pub fn id(&self) -> &str {
        &self.job.id
    }

    /// `ok downloading <bytes> <of>`, then `ok storing <bytes> <of>`.
    pub fn progress(&self) -> String {
        let length = self.job.length;
        match &self.phase {
            Phase::Fetching(run) => {
                let done = (run.progress() as u64)
                    .saturating_mul(fetch::MODEL_CHUNK)
                    .min(length);
                format!("ok downloading {done} {length}")
            }
            Phase::Copying(at) => format!("ok storing {at} {length}"),
        }
    }

    /// The next step: a chunk fetched or read back, or a chunk copied into
    /// the store; `Some` once the model is stored.
    pub fn step(&mut self, store: Handle) -> Result<Option<Done>, String> {
        if sys::time_ns() < self.wait_until_ns {
            return Ok(None);
        }
        match self.phase {
            Phase::Fetching(_) => self.fetch_step(store).map(|()| None),
            Phase::Copying(at) => self.copy_step(store, at),
        }
    }

    fn fetch_step(&mut self, store: Handle) -> Result<(), String> {
        let Phase::Fetching(run) = &mut self.phase else {
            return Ok(());
        };
        let job = &self.job;
        let url = Url::parse(&job.source.url()).map_err(|why| format!("the source: {why}"))?;
        let deadline_ns = sys::time_ns().saturating_add(CHUNK_NS);
        let mut connect = TlsConnect {
            config: self.config.clone(),
            dial: NetDial {
                dest: self.dest,
                bulk: true,
                deadline_ns,
            },
        };
        let allow = |host: &str, port: u16| job.reaches(host, port);
        let clock = || sys::time_ns();
        let request = Fetch {
            limits: Limits::new(fetch::MODEL_CHUNK, deadline_ns),
            allow: &allow,
            clock: &clock,
        };
        let mut fetcher = |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
            let got = request.run(&mut connect, Method::Get, &url, Some(range), sink)?;
            Ok(Answer::from(&got))
        };
        match run.step(&mut self.partial, &mut fetcher) {
            Ok(Step::Checked(_) | Step::Fetched(_)) => {
                self.tries = 0;
                Ok(())
            }
            Ok(Step::Done(report)) => {
                self.report = report;
                self.begin_copy(store)
            }
            Err(DownloadError::Whole) => Err(String::from(
                "every chunk matched its list, but the whole model's SHA-256 did not",
            )),
            Err(error) => self.retry(&error),
        }
    }

    /// A failed step: the run stays where it was, so the next try asks for
    /// the same chunk again, after a wait that doubles, or as long as a 429
    /// or 503 says.
    fn retry(&mut self, error: &DownloadError) -> Result<(), String> {
        self.tries += 1;
        let why = self.partial.why(error);
        if self.tries > MAX_TRIES {
            return Err(format!("{MAX_TRIES} tries in a row failed: {why}"));
        }
        let doubled = FIRST_WAIT_NS.saturating_mul(1u64 << (self.tries - 1).min(16));
        let wait = match error {
            DownloadError::Status(_, Some(seconds)) => u64::from(*seconds) * 1_000_000_000,
            _ => doubled,
        };
        println!(
            "updated: model download, try {} of {MAX_TRIES} again: {why}",
            self.tries
        );
        self.wait_until_ns = sys::time_ns().saturating_add(wait.min(MAX_WAIT_NS));
        Ok(())
    }

    /// The whole model checked: if the disk holds a second copy, the store's
    /// sealed blob is begun in an action of its own.
    fn begin_copy(&mut self, store: Handle) -> Result<(), String> {
        let length = self.job.length;
        let free = match folders::ask(store, &Request::Df)? {
            (Reply::Space { free, .. }, _) => free.saturating_mul(4096),
            other => return Err(format!("the store's free space: {other:?}")),
        };
        let need = fetch::disk_need(length, length);
        if need > free {
            return Err(fetch::Refused::Disk { need, free }.why());
        }
        let done = |request: Request| match folders::ask(store, &request)? {
            (Reply::Done, _) => Ok(()),
            other => Err(format!("the store: {other:?}")),
        };
        done(Request::Begin { action: 0 })?;
        let begin = Request::SealedBegin {
            name: format!("{}.gguf", hex(&self.job.sha256)),
            len: length,
        };
        if let Err(why) = done(begin) {
            let _ = folders::ask(store, &Request::Abort);
            return Err(why);
        }
        self.phase = Phase::Copying(0);
        Ok(())
    }

    /// The next chunk of the checked file pushed into the sealed blob; at the
    /// end, the blob finished and committed and the partial file removed.
    fn copy_step(&mut self, store: Handle, at: u64) -> Result<Option<Done>, String> {
        let length = self.job.length;
        if at < length {
            let n = COPY_STEP.min(length - at);
            let mut buf = vec![0u8; n as usize];
            dl::Partial::read(&mut self.partial, at, &mut buf).map_err(|e| self.partial.why(&e))?;
            for piece in buf.chunks(MAX_DATA) {
                let push = Request::SealedPush {
                    data: piece.to_vec(),
                };
                match folders::ask(store, &push)? {
                    (Reply::Done, _) => {}
                    other => return Err(format!("the store: {other:?}")),
                }
            }
            self.phase = Phase::Copying(at + n);
            return Ok(None);
        }
        match folders::ask(store, &Request::SealedFinish)? {
            (Reply::Done, _) => {}
            other => return Err(format!("the store refused the model: {other:?}")),
        }
        match folders::ask(store, &Request::Commit)? {
            (Reply::Committed { .. }, _) => {}
            other => return Err(format!("the store: {other:?}")),
        }
        self.remove_partial();
        let elapsed_ns = sys::time_ns().saturating_sub(self.started_ns).max(1);
        Ok(Some(Done {
            id: self.job.id.clone(),
            line: self.line(elapsed_ns),
        }))
    }

    /// The partial file and its mark, once the store keeps the model: the
    /// file is cut to nothing first, so its blocks are free even while the
    /// volume's trash holds its name.
    fn remove_partial(&mut self) {
        let held = &mut self.partial.0;
        held.close();
        let (dir, partial, mark) = (held.dir(), held.partial_name(), held.mark_name());
        let rw = Rights::READ | Rights::WRITE;
        if let Ok(Some(file)) = kaifs::open(dir, &partial, How::Existing, rw) {
            if let Ok((other, _)) = kaifs::ask(file, Request::Truncate { size: 0 })
                && other != Reply::Done
            {
                println!("updated: {partial} was not cut: {other:?}");
            }
            let _ = sys::handle_close(file);
        }
        for name in [partial, mark] {
            match kaifs::ask(dir, Request::Trash { name: name.clone() }) {
                Ok((Reply::Done, _)) => {}
                other => println!("updated: {name} stays in {PARTIAL_IN_STORE}: {other:?}"),
            }
        }
    }

    /// How the download went, for the log and the case: the bytes fetched
    /// this run over the whole time, the check and the copy included, in
    /// MB/s. `kainet` closes each connection, so every chunk is a TLS
    /// connection of its own.
    fn line(&self, elapsed_ns: u64) -> String {
        let kept = (self.report.kept as u64).saturating_mul(fetch::MODEL_CHUNK);
        let bytes = self.job.length.saturating_sub(kept);
        let tenths = u128::from(bytes) * 10_000 / u128::from(elapsed_ns);
        format!(
            "updated: downloaded {} ({}), {} bytes, in {} ms: {}.{} MB/s ({} chunks fetched, {} kept, a TLS connection each)",
            self.job.id,
            &hex(&self.job.sha256)[..12],
            self.job.length,
            elapsed_ns / 1_000_000,
            tenths / 10,
            tenths % 10,
            self.report.fetched,
            self.report.kept
        )
    }

    /// Drops the run: an open copy's action is aborted; what `.partial/`
    /// holds stays for the next try.
    pub fn abandon(&mut self, store: Handle) {
        if matches!(self.phase, Phase::Copying(_)) {
            let _ = folders::ask(store, &Request::Abort);
        }
        self.partial.0.close();
    }
}
