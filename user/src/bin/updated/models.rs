//! The model channel in `updated` (ADR 0008 §6 and §11; P4 task 10): the list,
//! the import from the import disk, `model_use` and the trial a used model is
//! on. What it decides is `modelchan`'s; this holds the handles: the `Update`
//! handle for the import disk (`kabi::update::IMPORT`), the model store's `Dir`
//! (`dir=models`), `/system/updates` for `kai-modeltrial 1`, the image's
//! `pkgrepo/` for the signed `models` role, and a route to `llm` for its health.
//!
//! Only signed models (K, 2026-10-30): an import is kept only if it is a target
//! of the role by length and SHA-256 (`fs` checks the hash again before it
//! keeps the blob), and `model_use` takes only a stored blob the role signs
//! under a licence the project takes.
//!
//! An import holds `fs`'s one open action until the blob is whole, so other
//! writers wait meanwhile; models are imported rarely, and a 2.5 GB one takes
//! about 357,000 pushes of 7 KiB.

use crate::folders::{self, Folders};
use crate::{download, offers};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use comp_models::{Models, Refusal, Sha256, Signed, Store, Stored, hex};
use fsproto::{Cursor, Kind, MAX_DATA, Reply, Request};
use kabi::Error;
use kabi::handle::Handle;
use kabi::update::{IMPORT, MAX_IO, SECTOR};
use kaipkg::image::Files;
use kaituf::meta::TargetFile;
use modelchan::fetch::Hosts;
use modelchan::import::{self, Importing};
use modelchan::listing;
use modelchan::trial::{self, AtBoot, Decision, Health, State, Switch, Trial};
use user::{println, sys};

/// The trial record, in `/system/updates`.
const TRIAL: &str = "model-trial";
/// How long a boot waits for `llm` to load a candidate: ADR 0008 §9's limit
/// for a trial slot to come up.
const PROBATION_MS: u64 = 300_000;
/// How often the probation asks `llm`, and how long it waits for an answer.
const HEALTH_EVERY_MS: u64 = 5_000;
const HEALTH_CALL_MS: u64 = 3_000;
/// Names listed at a time from the store.
const LIST_PAGE: u32 = 64;

/// The image's repository, read with BOOT_READ (`bootfiles=pkgrepo`), as `pkg`
/// reads it.
struct BootRepo;

impl Files for BootRepo {
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let path = format!("pkgrepo/{path}");
        let len = match sys::boot_read(&path, &mut []) {
            Ok(len) => len,
            Err(Error::NotFound) => return Ok(None),
            Err(e) => return Err(format!("BOOT_READ {path}: {e:?}")),
        };
        if len as u64 > max {
            return Err(format!("{path} is {len} bytes, over {max}"));
        }
        let mut bytes = vec![0u8; len];
        match sys::boot_read(&path, &mut bytes) {
            Ok(got) if got == len => Ok(Some(bytes)),
            Ok(_) => Err(format!("{path} changed while it was read")),
            Err(e) => Err(format!("BOOT_READ {path}: {e:?}")),
        }
    }
}

/// The signed `models` role, as the import and the list hold models to it.
fn role() -> Result<Vec<Signed>, String> {
    role_and_lists().map(|(signed, _)| signed)
}

/// The chunk lists the role signs, each by its model's SHA-256.
type Lists = Vec<(Sha256, TargetFile)>;

/// The role's models, and the chunk lists it signs beside them, each by its
/// model's SHA-256 (the download fetches a list and holds it to its target).
fn role_and_lists() -> Result<(Vec<Signed>, Lists), String> {
    let checked = kaipkg::models::check(&mut BootRepo)?;
    for (path, why) in &checked.refused {
        println!("updated: the models role's {path} is refused: {why}");
    }
    let signed = checked.models.into_iter().map(|m| Signed {
        id: m.id,
        name: m.name,
        length: m.length,
        sha256: m.sha256,
        licence: m.licence,
        source: m.source,
    });
    Ok((signed.collect(), checked.chunks))
}

/// The store's name for a blob: `<64 hex>.gguf`.
fn blob(sha256: &Sha256) -> String {
    format!("{}.gguf", hex(sha256))
}

/// An import under way: the disk read so far, and the blob it fills.
struct Running {
    importing: Importing,
    sha256: Sha256,
    /// The disk bytes read: whole sectors.
    at: u64,
}

pub struct Channel {
    update: Handle,
    store: Handle,
    updates: Handle,
    llm: Option<Handle>,
    import: Option<Running>,
    /// The last import's end: the target it stored, or why it was refused.
    last: Option<Result<String, String>>,
    /// The trial this boot judges, and when it started.
    probation: Option<(Trial, u64)>,
    asked_ms: u64,
    /// The hosts models are downloaded from (`offers::hosts`), and where the
    /// role's chunk lists are (`download::lists`).
    hosts: &'static [Hosts],
    lists: String,
    download: Option<download::Running>,
    /// The last download's end: the target it stored, or why it stopped.
    downloaded: Option<Result<String, String>>,
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

impl Channel {
    /// The channel, when the plan line grants the model store and `kai-data`
    /// serves it.
    pub fn open(
        update: Handle,
        folders: &Folders,
        llm: Option<Handle>,
        hosts: &'static [Hosts],
        lists: String,
    ) -> Option<Channel> {
        Some(Channel {
            update,
            store: folders.models?,
            updates: folders.updates?,
            llm,
            import: None,
            last: None,
            probation: None,
            asked_ms: 0,
            hosts,
            lists,
            download: None,
            downloaded: None,
        })
    }

    fn record(&self) -> Result<Option<Trial>, String> {
        let Some(bytes) = folders::read(self.updates, TRIAL)? else {
            return Ok(None);
        };
        if bytes.is_empty() {
            return Ok(None);
        }
        let text = core::str::from_utf8(&bytes).map_err(|_| format!("{TRIAL} is not text"))?;
        Trial::parse(text).map(Some)
    }

    fn keep(&self, trial: Option<&Trial>) -> Result<(), String> {
        let text = trial.map_or_else(String::new, Trial::encode);
        folders::write(self.updates, TRIAL, text.as_bytes())
    }

    /// Points `kai-modelhint 1` at `model`, or at none.
    fn hint(&self, model: Option<&Sha256>) -> Result<(), String> {
        let name = model.map(blob);
        match folders::ask(self.store, &Request::ModelHint { name })? {
            (Reply::Done, _) => Ok(()),
            other => Err(format!("the model hint: {other:?}")),
        }
    }

    /// The first thing a boot does with the last trial, before it waits on
    /// anything (K's condition 3): a pending trial is marked attempted, and
    /// one already attempted, which a hang cut short, goes back at once.
    pub fn at_boot(&mut self) -> Result<(), String> {
        match trial::at_boot(self.record()?.as_ref()) {
            AtBoot::Nothing => {}
            AtBoot::Try(attempted) => {
                self.keep(Some(&attempted))?;
                println!(
                    "updated: model {} is on trial",
                    &hex(&attempted.candidate)[..12]
                );
                self.probation = Some((attempted, now_ms()));
            }
            AtBoot::RollBack(back) => self.roll_back(back)?,
        }
        Ok(())
    }

    fn roll_back(&mut self, trial: Trial) -> Result<(), String> {
        self.hint(trial.previous.as_ref())?;
        self.keep(Some(&trial))?;
        if let State::RolledBack(why) = &trial.state {
            println!(
                "updated: model {} rolled back: {why}",
                &hex(&trial.candidate)[..12]
            );
        }
        Ok(())
    }

    /// Work between requests: the probation's next look at `llm`, the
    /// running import's next read, and the running download's next chunk.
    pub fn tick(&mut self) {
        if let Err(why) = self.judge() {
            println!("updated: model trial: {why}");
        }
        if self.import.is_some() {
            let step = self.step();
            if let Err(why) = step {
                self.end_import(Err(why));
            }
        }
        if let Some(run) = &mut self.download {
            match run.step(self.store) {
                Ok(None) => {}
                Ok(Some(done)) => {
                    println!("{}", done.line);
                    self.download = None;
                    self.end_download(Ok(done.id));
                }
                Err(why) => {
                    run.abandon(self.store);
                    self.download = None;
                    self.end_download(Err(why));
                }
            }
        }
    }

    fn end_download(&mut self, end: Result<String, String>) {
        if let Err(why) = &end {
            println!("updated: model download stopped: {why}");
        }
        self.last = None;
        self.downloaded = Some(end);
    }

    /// Whether there is work between requests, so the serve loop does not
    /// sleep long.
    pub fn busy(&self) -> bool {
        self.import.is_some() || self.download.is_some() || self.probation.is_some()
    }

    /// Why a change to the store must wait: an import or a download holds
    /// it, or will hold `fs`'s one action for its copy.
    fn store_busy(&self) -> Option<String> {
        if self.import.is_some() {
            return Some(String::from("an import is running; ask again once it ends"));
        }
        let run = self.download.as_ref()?;
        Some(format!(
            "{} is downloading; ask again once it ends",
            run.id()
        ))
    }

    /// `models download <id>`: the signed target `id` from its pinned
    /// source (`download.rs`), once nothing else changes the store.
    fn start_download(&mut self, id: &str) -> Result<String, String> {
        if let Some(why) = self.store_busy() {
            return Err(why);
        }
        // As for an import: a trial's verdict writes its record, which the
        // copy's open action would block.
        if self.probation.is_some() {
            return Err(String::from(
                "a model is on trial; ask again once it is kept or rolled back",
            ));
        }
        let (signed, lists) = role_and_lists()?;
        let store = Store {
            blobs: self
                .blobs()?
                .iter()
                .map(|&(sha256, length, _)| Stored { sha256, length })
                .collect(),
            ..Store::default()
        };
        let facts = offers::Facts::read(self.store, &signed)?;
        let machine = facts.machine(&store, self.hosts);
        let run = download::start(self.store, &signed, &lists, &machine, id, &self.lists)?;
        let target = signed
            .iter()
            .find(|t| t.id == run.id())
            .ok_or("the role changed")?;
        let host = modelchan::fetch::Source::parse(&target.source)
            .map(|s| s.host)
            .unwrap_or_default();
        let reply = format!("ok downloading {} {} {host}", target.id, target.length);
        println!("updated: downloading {} from {host}", target.id);
        self.download = Some(run);
        Ok(reply)
    }

    /// Whether a model download is under way.
    pub fn downloading(&self) -> bool {
        self.download.is_some()
    }

    /// Whether a model is on trial, its probation not yet decided.
    pub fn on_trial(&self) -> bool {
        self.probation.is_some()
    }

    /// Leaves `case-<step>.done` in `/system/updates` for the kernel's half of
    /// the `models` case, which ends the boot only once it is there.
    pub fn mark_done(&self, step: &str) -> Result<(), String> {
        folders::write(self.updates, &format!("case-{step}.done"), b"")
    }

    fn judge(&mut self) -> Result<(), String> {
        let Some((trial, started)) = self.probation.clone() else {
            return Ok(());
        };
        let now = now_ms();
        if now.saturating_sub(self.asked_ms) < HEALTH_EVERY_MS {
            return Ok(());
        }
        self.asked_ms = now;
        let reply = self.health();
        let health = match reply.as_deref() {
            None => Health::NotYet,
            Some(text) => health_of(text),
        };
        match trial::decide(trial.fnv, health, now - started, PROBATION_MS) {
            Decision::Wait => Ok(()),
            Decision::Keep => {
                let kept = Trial {
                    state: State::Kept,
                    ..trial
                };
                self.probation = None;
                self.keep(Some(&kept))?;
                println!("updated: model {} kept", &hex(&kept.candidate)[..12]);
                Ok(())
            }
            Decision::RollBack(why) => {
                let back = Trial {
                    state: State::RolledBack(why),
                    ..trial
                };
                self.probation = None;
                self.roll_back(back)
            }
        }
    }

    /// `llm`'s `health` reply, or none when it does not answer in time.
    fn health(&self) -> Option<String> {
        let llm = self.llm?;
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(llm, b"health", &mut reply, HEALTH_CALL_MS).ok()?;
        String::from_utf8(reply[..len.min(reply.len())].to_vec()).ok()
    }

    /// One request from the shell: `models`, `models import`, `models
    /// progress`, `models use <64 hex>`, `models offers`, `models ask <id>` or
    /// `models download <id>`; from the model manager's window, `models` alone
    /// (`serve` checks who asked). Nothing in it is used as a path: an id is
    /// only looked up in the signed role.
    pub fn answer(&mut self, text: &str) -> String {
        let done = match text.split(' ').collect::<Vec<_>>().as_slice() {
            ["models"] => self.list(),
            ["models", "import"] => self.start_import(),
            ["models", "progress"] => Ok(self.progress()),
            ["models", "use", sha] => listing::sha256(sha).and_then(|sha| self.use_model(&sha)),
            ["models", "offers"] => self.offers(None),
            ["models", "ask", id] => self.offers(Some(id)),
            ["models", "download", id] => self.start_download(id),
            _ => Err(String::from("unknown request")),
        };
        done.unwrap_or_else(|why| format!("err {why}"))
    }

    /// `models offers`, or with `id`, `models ask <id>` (`offers.rs`).
    fn offers(&self, id: Option<&str>) -> Result<String, String> {
        let signed = role()?;
        let store = Store {
            blobs: self
                .blobs()?
                .iter()
                .map(|&(sha256, length, _)| Stored { sha256, length })
                .collect(),
            ..Store::default()
        };
        let facts = offers::Facts::read(self.store, &signed)?;
        let machine = facts.machine(&store, self.hosts);
        match id {
            None => Ok(offers::offers(&signed, &machine)),
            Some(id) => offers::ask(&signed, &machine, id),
        }
    }

    /// The store's sealed blobs: each one's SHA-256, length and FNV-1a64.
    fn blobs(&self) -> Result<Vec<(Sha256, u64, u64)>, String> {
        let mut out = Vec::new();
        let mut after: Option<Cursor> = None;
        loop {
            let request = Request::List {
                after: after.clone(),
                max: LIST_PAGE,
            };
            let (rows, more) = match folders::ask(self.store, &request)? {
                (Reply::Listed { names, more }, _) => (names, more),
                other => return Err(format!("listing the store: {other:?}")),
            };
            for row in &rows {
                let Some(sha) = fsserve::models::blob_sha(&row.name) else {
                    continue;
                };
                if row.kind != Kind::File {
                    continue;
                }
                let info = Request::ModelInfo {
                    name: row.name.clone(),
                };
                if let (Reply::Model { len, fnv }, _) = folders::ask(self.store, &info)? {
                    out.push((sha, len, fnv));
                }
            }
            match more {
                Some(cursor) => after = Some(cursor),
                None => return Ok(out),
            }
        }
    }

    /// What the model manager shows: the role, the store, the running and
    /// next models and the last trial, as `comp_models` holds them.
    fn models(&self) -> Result<Models, String> {
        let signed = role()?;
        let blobs = self.blobs()?;
        let running = self
            .health()
            .and_then(|text| listing::loaded(&text))
            .and_then(|(len, fnv)| blobs.iter().find(|b| (b.1, b.2) == (len, fnv)))
            .map(|b| b.0);
        let record = self.record()?;
        let next = record
            .as_ref()
            .filter(|t| t.state == State::Pending)
            .map(|t| t.candidate);
        let rolled_back = record.as_ref().and_then(|t| match &t.state {
            State::RolledBack(why) => Some((t.candidate, why.clone())),
            _ => None,
        });
        Ok(Models {
            signed,
            store: Store {
                blobs: blobs
                    .iter()
                    .map(|&(sha256, length, _)| Stored { sha256, length })
                    .collect(),
                running,
                next,
                on_trial: self.probation.is_some(),
                rolled_back,
            },
            ..Models::default()
        })
    }

    /// The list as one message holds it (`listing::encode_within`).
    fn list(&self) -> Result<String, String> {
        self.models()
            .map(|m| listing::encode_within(&m, kabi::MAX_MESSAGE))
    }

    /// `model_use`: the stored, signed model `sha256` from the next restart,
    /// on trial; or back to the running one before it.
    fn use_model(&mut self, sha256: &Sha256) -> Result<String, String> {
        // The record and the hint are written outside an action, which the
        // import holds until its blob is whole (K's review of #447).
        if let Some(why) = self.store_busy() {
            println!("updated: models use refused: {why}");
            return Err(why);
        }
        let models = self.models()?;
        if models.use_model(sha256).is_none() && models.store.running != Some(*sha256) {
            return Err(format!(
                "{} is not a stored model the signed role takes",
                &hex(sha256)[..12]
            ));
        }
        let fnv = match folders::ask(self.store, &Request::ModelInfo { name: blob(sha256) })? {
            (Reply::Model { fnv, .. }, _) => fnv,
            other => return Err(format!("the store's {}: {other:?}", blob(sha256))),
        };
        let running = models.store.running;
        match trial::switch(running, self.record()?.as_ref(), *sha256, fnv) {
            Switch::Nothing => Ok(String::from("ok already")),
            // The record first: a boot that finds a hint and no record would
            // try the candidate with nothing to go back to.
            Switch::Try(pending) => {
                self.keep(Some(&pending))?;
                self.hint(Some(sha256))?;
                Ok(format!("ok next {}", hex(sha256)))
            }
            Switch::Cancel => {
                self.hint(running.as_ref())?;
                self.keep(None)?;
                Ok(String::from("ok cancelled"))
            }
        }
    }

    fn start_import(&mut self) -> Result<String, String> {
        if let Some(why) = self.store_busy() {
            return Err(why);
        }
        // A trial's verdict writes its record, which an import's open action
        // would block, so an import waits for the trial to end.
        if self.probation.is_some() {
            println!("updated: models import refused: a model is on trial");
            return Err(String::from(
                "a model is on trial; ask again once it is kept or rolled back",
            ));
        }
        let info = sys::slot_info(self.update, IMPORT).map_err(|_| "no import disk")?;
        let signed = role()?;
        let refused = |why: Refusal| why.why();
        let importing = import::start(&signed, info.sectors * SECTOR).map_err(refused)?;
        let sha256 = signed
            .iter()
            .find(|s| s.id == importing.id())
            .map(|s| s.sha256)
            .ok_or("the role changed")?;
        let begin = |request: Request| match folders::ask(self.store, &request)? {
            (Reply::Done, _) => Ok(()),
            other => Err(format!("{other:?}")),
        };
        begin(Request::Begin { action: 0 })?;
        let sealed = Request::SealedBegin {
            name: blob(&sha256),
            len: importing.length(),
        };
        if let Err(why) = begin(sealed) {
            let _ = folders::ask(self.store, &Request::Abort);
            return Err(format!("the store: {why}"));
        }
        let reply = format!("ok importing {} {}", importing.id(), importing.length());
        self.last = None;
        self.import = Some(Running {
            importing,
            sha256,
            at: 0,
        });
        Ok(reply)
    }

    /// The running import's next disk read, pushed to the store; at the end,
    /// the blob finished and committed, or the action aborted.
    fn step(&mut self) -> Result<(), String> {
        let Some(run) = &mut self.import else {
            return Ok(());
        };
        let disk = run.importing.disk();
        if run.at < disk {
            let len = MAX_IO.min(disk - run.at);
            let mut buf = vec![0u8; len as usize];
            sys::slot_read(self.update, IMPORT, run.at / SECTOR, &mut buf)
                .map_err(|e| format!("SLOT_READ: {e:?}"))?;
            run.at += len;
            let blob = run.importing.push(&buf).map_err(|why| why.why())?;
            for piece in blob.chunks(MAX_DATA) {
                let push = Request::SealedPush {
                    data: piece.to_vec(),
                };
                match folders::ask(self.store, &push)? {
                    (Reply::Done, _) => {}
                    other => return Err(format!("the store: {other:?}")),
                }
            }
            return Ok(());
        }
        let run = self.import.take().ok_or("no import")?;
        let signed = role()?;
        let imported = run.importing.finish(&signed).map_err(|why| why.why())?;
        if imported.sha256 != run.sha256 {
            return Err(String::from("the role changed"));
        }
        match folders::ask(self.store, &Request::SealedFinish)? {
            (Reply::Done, _) => {}
            other => return Err(format!("the store refused the blob: {other:?}")),
        }
        match folders::ask(self.store, &Request::Commit)? {
            (Reply::Committed { .. }, _) => {}
            other => return Err(format!("the store: {other:?}")),
        }
        println!(
            "updated: imported {} ({}), {} bytes",
            imported.id,
            &hex(&imported.sha256)[..12],
            imported.length
        );
        self.end_import(Ok(imported.id));
        Ok(())
    }

    fn end_import(&mut self, end: Result<String, String>) {
        if self.import.take().is_some() || end.is_err() {
            // A refused import leaves nothing: its action is aborted.
            let _ = folders::ask(self.store, &Request::Abort);
        }
        if let Err(why) = &end {
            println!("updated: import refused: {why}");
        }
        self.downloaded = None;
        self.last = Some(end);
    }

    /// `ok <read> <of>` while an import runs; then how the last one ended.
    fn progress(&self) -> String {
        if let Some(run) = &self.download {
            return run.progress();
        }
        match &self.downloaded {
            Some(Ok(id)) if self.last.is_none() => return format!("ok downloaded {id}"),
            Some(Err(why)) if self.last.is_none() => return format!("ok refused {why}"),
            _ => {}
        }
        match (&self.import, &self.last) {
            (Some(run), _) => format!("ok {} {}", run.at, run.importing.disk()),
            (None, Some(Ok(id))) => format!("ok imported {id}"),
            (None, Some(Err(why))) => format!("ok refused {why}"),
            (None, None) => String::from("ok none"),
        }
    }
}

/// `llm`'s `health` reply as the probation reads it.
fn health_of(text: &str) -> Health<'_> {
    use llmproto::health::{State as Llm, Status};
    let state = Status::of(text).map(|s| s.state);
    match state {
        Some(Llm::Ready | Llm::Warming) => match listing::loaded(text) {
            Some((_, fnv)) => Health::Ready { fnv },
            None => Health::Failed("its health names no model"),
        },
        Some(Llm::Failed) => Health::Failed(text),
        Some(Llm::NoModel | Llm::Remote) => Health::NoModel,
        Some(Llm::Waiting | Llm::Busy) => Health::NotYet,
        None if text.starts_with("err no model") => Health::NoModel,
        None => Health::NotYet,
    }
}
