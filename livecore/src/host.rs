//! The component host: one component, one request at a time, so the point between
//! two requests is always a safe place to swap versions.
//!
//! It serves with the active version, checkpointing the state to the kernel after
//! every request that changed it. A swap stages a candidate, migrates the state
//! into it and checks it, then puts it on probation: each request runs on the old
//! version first (its state is what the kernel keeps), then on the candidate, whose
//! reply is sent. Any fault rolls back at once and loses nothing, because the old
//! version already handled the request. Rollbacks come only from faults and failed
//! checks, never from time, so a slow machine cannot cause one.
//!
//! Each call is one transaction (`txn`): its storage writes commit with the
//! checkpoint and its effects go out in the reply, or neither does.

use crate::artifact::{self, Manifest};
use crate::call::REPLY_HEADER;
use crate::control;
use crate::settings::Settings;
use crate::txn::{self, Inputs, Storage, Txn};
use crate::undo::{History, Saved};
use crate::{Fault, Instance, Reason};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{flag, kind};

/// What the host needs from the kernel and the runtime.
pub trait Env {
    type Instance: Instance;
    /// A version's artifact from the store.
    fn fetch(&mut self, label: &str) -> Result<Vec<u8>, Error>;
    /// Reports a ledger event; the kernel refuses what its rules do not allow.
    fn report(&mut self, event: &Report<'_>) -> Result<(), Error>;
    /// The ledger and the active version's checkpoint.
    fn restore(&mut self) -> Result<Restored, Error>;
    /// Compiles and instantiates a module; `quiet` drops its log lines.
    fn load(&mut self, module: &[u8], quiet: bool) -> Result<Self::Instance, Fault>;
    fn now_ms(&self) -> u64;
    fn log(&mut self, text: &str);
}

/// An event for the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report<'a> {
    pub kind: u64,
    pub reason: Option<Reason>,
    pub label: &'a str,
    pub schema: u64,
    pub served: u64,
    pub regressions: u64,
    pub flags: u64,
    /// A state; the host joins the app's storage to it before it goes out.
    pub blob: &'a [u8],
    /// The storage to join to `blob`; the host's own when `None`.
    pub storage: Option<&'a Storage>,
    pub detail: &'a str,
}

impl Report<'_> {
    /// The event as the COMP_EVENT call takes it, pointing into this report.
    pub fn to_raw(&self) -> kabi::live::Event {
        kabi::live::Event {
            kind: self.kind,
            reason: self.reason.map_or(0, |reason| reason as u64),
            label_ptr: self.label.as_ptr() as u64,
            label_len: self.label.len() as u64,
            schema: self.schema,
            served: self.served,
            regressions: self.regressions,
            flags: self.flags,
            blob_ptr: self.blob.as_ptr() as u64,
            blob_len: self.blob.len() as u64,
            detail_ptr: self.detail.as_ptr() as u64,
            detail_len: self.detail.len() as u64,
        }
    }

    pub const fn new(kind: u64) -> Self {
        Self {
            kind,
            reason: None,
            label: "",
            schema: 0,
            served: 0,
            regressions: 0,
            flags: 0,
            blob: &[],
            storage: None,
            detail: "",
        }
    }
}

/// The ledger as the host starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// The component, named like the host process.
    pub name: String,
    pub phase: u64,
    pub schema: u64,
    pub disabled: bool,
    pub active: String,
    pub candidate: String,
    pub checkpoint: Vec<u8>,
}

/// Why the host must end; its supervisor restarts it, and it restores from the
/// kernel's ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub why: String,
}

/// Exit code when the active version cannot be loaded.
pub const EXIT_NO_VERSION: i32 = 3;
/// Exit code when the active version keeps failing.
pub const EXIT_RESTARTS: i32 = 2;
/// Exit code when the kernel refused an event.
pub const EXIT_REFUSED: i32 = 4;
/// In-place reloads allowed within [`RESTART_WINDOW_MS`] before the host gives up.
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW_MS: u64 = 60_000;
/// Requests kept for `ctl try` to replay: the last ones served without a fault.
pub const REPLAY: usize = 8;

/// A loaded version.
pub(crate) struct Version<I> {
    pub label: String,
    pub schema: u32,
    pub manifest: Manifest,
    /// Kept for reloads and `ctl source`.
    pub artifact: Vec<u8>,
    pub instance: I,
}

pub(crate) struct Probation<I> {
    pub standby: Version<I>,
    pub candidate: Version<I>,
    pub began_ms: u64,
    pub served: u64,
    pub regressions: u64,
    pub health_ok: u64,
}

pub(crate) enum Mode<I> {
    Serving(Version<I>),
    Probation(Box<Probation<I>>),
    /// The checkpoint could not be restored; waits for `ctl reset`.
    Disabled {
        version: Version<I>,
        why: String,
    },
    /// Suspended: its state saved, its instance dropped (`ctl suspend`).
    Suspended(crate::suspend::Parked),
    /// Only while a request moves the host from one mode to another.
    Moving,
}

/// What the host keeps besides its mode.
pub(crate) struct Core<E> {
    pub env: E,
    pub name: String,
    pub settings: Settings,
    /// The active version's state, as the kernel holds it (without the storage).
    pub checkpoint: Vec<u8>,
    /// The app's own scope, as the kernel holds it with the checkpoint.
    pub storage: Storage,
    restarts: Vec<u64>,
    /// The last [`REPLAY`] requests served, oldest first.
    pub recent: VecDeque<Vec<u8>>,
    /// Calls served, which make each call's seed.
    calls: u64,
    /// The ticket the next performed effect gets.
    next_ticket: u32,
    /// Undo for the last calls that changed the state.
    pub history: History,
}

pub struct Host<E: Env> {
    pub(crate) core: Core<E>,
    pub(crate) mode: Mode<E::Instance>,
}

/// A failure inside a step: a version's fault, or a reason to end the host.
pub(crate) enum Stop {
    Fault(Fault),
    Exit(Exit),
}

impl From<Fault> for Stop {
    fn from(fault: Fault) -> Self {
        Stop::Fault(fault)
    }
}

impl From<Exit> for Stop {
    fn from(exit: Exit) -> Self {
        Stop::Exit(exit)
    }
}

pub(crate) fn text(reply: impl Into<String>) -> Vec<u8> {
    reply.into().into_bytes()
}

pub(crate) fn exit(code: i32, why: impl Into<String>) -> Exit {
    Exit {
        code,
        why: why.into(),
    }
}

impl<E: Env> Core<E> {
    pub(crate) fn new(env: E, name: &str, settings: Settings) -> Self {
        Self {
            env,
            name: name.to_string(),
            settings,
            checkpoint: Vec::new(),
            storage: Storage::new(),
            restarts: Vec::new(),
            recent: VecDeque::new(),
            calls: 0,
            next_ticket: 1,
            history: History::default(),
        }
    }

    /// Sends an event, its state joined with the storage.
    pub fn send(&mut self, event: &Report<'_>) -> Result<(), Error> {
        if event.blob.is_empty() {
            return self.env.report(event);
        }
        let storage = event.storage.unwrap_or(&self.storage);
        let saved = crate::state::join(event.blob, storage).map_err(|_| Error::Invalid)?;
        let joined = Report {
            blob: &saved,
            ..*event
        };
        self.env.report(&joined)
    }

    /// Reports an event; a refusal ends the host.
    pub fn report(&mut self, event: &Report<'_>) -> Result<(), Exit> {
        self.send(event).map_err(|error| {
            exit(
                EXIT_REFUSED,
                format!(
                    "the kernel refused a {} event: {error:?}",
                    kind::name(event.kind)
                ),
            )
        })
    }

    /// Fetches, checks and loads a version. `world` is the active version's, to
    /// which the candidate's must be compatible.
    pub fn stage(
        &mut self,
        label: &str,
        artifact: Vec<u8>,
        world: Option<&str>,
        quiet: bool,
    ) -> Result<Version<E::Instance>, Fault> {
        let manifest = artifact::parse(&artifact)
            .map_err(|why| Fault::new(Reason::Manifest, why))?
            .manifest;
        if manifest.component != self.name || manifest.version != label {
            return Err(Fault::new(
                Reason::Manifest,
                format!(
                    "the artifact is {} {}, not {} {label}",
                    manifest.component, manifest.version, self.name
                ),
            ));
        }
        if let Some(why) = manifest.unloadable() {
            return Err(Fault::new(Reason::Manifest, why));
        }
        if let Some(world) = world
            && !crate::manifest::compatible(world, &manifest.world)
        {
            return Err(Fault::new(
                Reason::Manifest,
                format!(
                    "world {} is not compatible with {world}; that needs a restart",
                    manifest.world
                ),
            ));
        }
        let mut instance = self.env.load(&artifact, quiet)?;
        instance.view(&self.storage);
        let schema = instance.schema()?;
        Ok(Version {
            label: label.to_string(),
            schema,
            manifest,
            artifact,
            instance,
        })
    }

    /// The inputs of the next call: the clock, a seed and its first ticket.
    pub fn inputs(&mut self) -> Inputs {
        self.calls += 1;
        let now_ms = self.env.now_ms();
        Inputs {
            now_ms,
            seed: txn::seed(now_ms, self.calls),
            first_ticket: self.next_ticket,
        }
    }

    /// Runs `request` on `version` as one transaction: its reply and what it did,
    /// checked against the limits. Over them is the version's fault.
    pub fn run(
        &mut self,
        version: &mut Version<E::Instance>,
        request: &[u8],
        inputs: Inputs,
        storage: &Storage,
    ) -> Result<(Vec<u8>, Txn), Fault> {
        version.instance.begin(storage, inputs);
        let reply = version.instance.handle(request);
        let txn = version.instance.finish();
        let reply = reply?;
        if txn.effects.len() > txn::MAX_EFFECTS {
            return Err(Fault::new(
                Reason::Abi,
                format!(
                    "it queued {} effects; at most {} a call",
                    txn.effects.len(),
                    txn::MAX_EFFECTS
                ),
            ));
        }
        for effect in &txn.effects {
            effect.check().map_err(|why| Fault::new(Reason::Abi, why))?;
        }
        let effects = txn.effect_lines(inputs.first_ticket);
        // Room for an `undo` line too.
        if reply.len() + effects.len() + 32 > kabi::MAX_MESSAGE {
            return Err(Fault::new(
                Reason::Abi,
                "its reply and effects are over one message",
            ));
        }
        Ok((reply, txn))
    }

    /// Commits `txn`'s writes with `version`'s state, sending the checkpoint if
    /// either changed; the undo entry for the change, if there was one.
    pub fn commit(
        &mut self,
        version: &mut Version<E::Instance>,
        txn: &Txn,
    ) -> Result<Option<u64>, Stop> {
        let blob = export_own(version)?;
        let mut storage = self.storage.clone();
        txn.apply(&mut storage);
        if blob == self.checkpoint && storage == self.storage {
            return Ok(None);
        }
        if crate::state::join(&blob, &storage)
            .map_or(true, |saved| saved.len() > kabi::live::MAX_CHECKPOINT)
        {
            return Err(Stop::Fault(Fault::new(
                Reason::Export,
                format!(
                    "its state and storage are over {} KiB",
                    kabi::live::MAX_CHECKPOINT / 1024
                ),
            )));
        }
        self.report(&Report {
            schema: u64::from(version.schema),
            blob: &blob,
            storage: Some(&storage),
            ..Report::new(kind::CHECKPOINT)
        })?;
        let before = Saved {
            state: core::mem::replace(&mut self.checkpoint, blob),
            storage: core::mem::replace(&mut self.storage, storage),
        };
        let after = Saved {
            state: self.checkpoint.clone(),
            storage: self.storage.clone(),
        };
        Ok(Some(self.history.record(before, after)))
    }

    /// The reply sent for a call whose effects are performed: after a
    /// `kai-reply`'s result come the effects, with their tickets, and the undo
    /// entry of the change it made.
    pub fn performed(
        &mut self,
        mut reply: Vec<u8>,
        txn: &Txn,
        inputs: Inputs,
        undo: Option<u64>,
    ) -> Vec<u8> {
        if !reply.starts_with(REPLY_HEADER.as_bytes()) {
            return reply;
        }
        if reply.last() != Some(&b'\n') {
            reply.push(b'\n');
        }
        reply.extend_from_slice(txn.effect_lines(inputs.first_ticket).as_bytes());
        self.next_ticket = inputs.first_ticket.wrapping_add(txn.effects.len() as u32);
        if let Some(undo) = undo {
            reply.extend_from_slice(format!("undo {undo}\n").as_bytes());
        }
        reply
    }

    /// A new instance of `version` holding the last checkpoint.
    pub fn reload(
        &mut self,
        version: &Version<E::Instance>,
        quiet: bool,
    ) -> Result<E::Instance, Fault> {
        let mut instance = self.env.load(&version.artifact, quiet)?;
        instance.view(&self.storage);
        match instance.import(&self.checkpoint, version.schema)? {
            0 => Ok(instance),
            code => Err(Fault::new(
                Reason::Migrate,
                format!("it refused its own checkpoint ({code})"),
            )),
        }
    }

    /// Keeps `request` for `ctl try`, dropping the oldest past [`REPLAY`].
    pub fn remember(&mut self, request: &[u8]) {
        if self.recent.len() == REPLAY {
            self.recent.pop_front();
        }
        self.recent.push_back(request.to_vec());
    }

    /// Counts an in-place reload; too many in a short time ends the host.
    pub fn note_restart(&mut self) -> Result<(), Exit> {
        let now = self.env.now_ms();
        self.restarts
            .retain(|&at| now.saturating_sub(at) < RESTART_WINDOW_MS);
        self.restarts.push(now);
        if self.restarts.len() > MAX_RESTARTS {
            return Err(exit(
                EXIT_RESTARTS,
                format!(
                    "{} failed {} times within a minute",
                    self.name,
                    self.restarts.len()
                ),
            ));
        }
        Ok(())
    }

    /// The active version failed: report it, and reload it from the checkpoint.
    pub fn reload_after(
        &mut self,
        version: &mut Version<E::Instance>,
        fault: &Fault,
        quiet: bool,
    ) -> Result<(), Exit> {
        self.report(&Report {
            reason: Some(fault.reason),
            flags: flag::RESTARTED,
            detail: &fault.detail,
            ..Report::new(kind::FAULT)
        })?;
        self.note_restart()?;
        version.instance = self.reload(version, quiet).map_err(|again| {
            exit(
                EXIT_RESTARTS,
                format!("{} could not be reloaded: {}", self.name, again.detail),
            )
        })?;
        Ok(())
    }
}

/// A version's own state, checked to be in its own schema.
pub(crate) fn export_own<I: Instance>(version: &mut Version<I>) -> Result<Vec<u8>, Fault> {
    let blob = version
        .instance
        .export(0)?
        .ok_or_else(|| Fault::new(Reason::Export, "it cannot export its own state"))?;
    match crate::state::schema_of(&blob) {
        Some(schema) if schema == u64::from(version.schema) => Ok(blob),
        found => Err(Fault::new(
            Reason::Export,
            format!("its state says schema {found:?}, not {}", version.schema),
        )),
    }
}

/// [`Host::start`] is in `start.rs`; `ctl` requests are in `control.rs`.
impl<E: Env> Host<E> {
    /// Answers one request from `caller` (0 is the kernel).
    pub fn handle(&mut self, caller: u64, request: &[u8]) -> Result<Vec<u8>, Exit> {
        let mode = core::mem::replace(&mut self.mode, Mode::Moving);
        let (mode, reply) = match request.strip_prefix(kabi::live::CONTROL_PREFIX.as_bytes()) {
            Some(_) if caller != 0 => {
                (mode, text("err control requests come only from the kernel"))
            }
            Some(rest) => {
                let rest = core::str::from_utf8(rest).unwrap_or("");
                match control::parse(rest) {
                    Ok(control) => self.control(mode, control)?,
                    Err(why) => (mode, text(format!("err {why}"))),
                }
            }
            None if request.starts_with(crate::undo::UNDO_HEADER.as_bytes()) => {
                let mode = match mode {
                    Mode::Suspended(_) => self.resume(mode).0,
                    other => other,
                };
                self.undo(mode, request)?
            }
            None => match mode {
                // A request wakes a suspended app first.
                Mode::Suspended(_) => match self.resume(mode) {
                    (Mode::Serving(version), _) => self.serve(version, request)?,
                    (mode, Err(why)) => (mode, text(why)),
                    (mode, Ok(_)) => (mode, text("err busy")),
                },
                Mode::Serving(version) => self.serve(version, request)?,
                Mode::Probation(probation) => {
                    crate::probation::dual_run(&mut self.core, *probation, request)?
                }
                Mode::Disabled { version, why } => {
                    let reply = format!(
                        "err {} disabled: cannot restore its state ({why}); ctl reset starts fresh",
                        self.core.name
                    );
                    (Mode::Disabled { version, why }, text(reply))
                }
                Mode::Moving => unreachable!("a request found the host between modes"),
            },
        };
        self.mode = mode;
        Ok(reply)
    }

    /// Called when no request came for [`Host::timeout_ms`]: checks a candidate.
    pub fn tick(&mut self) -> Result<(), Exit> {
        let mode = core::mem::replace(&mut self.mode, Mode::Moving);
        self.mode = match mode {
            Mode::Probation(probation) => crate::probation::tick(&mut self.core, *probation)?,
            other => other,
        };
        Ok(())
    }

    /// Runs `f` on each loaded instance with its version's label and whether
    /// its windows are the ones shown: the serving version, or in probation the
    /// candidate (the standby's are drained, never shown).
    pub fn each_instance(&mut self, mut f: impl FnMut(&mut E::Instance, &str, bool)) {
        match &mut self.mode {
            Mode::Serving(v) => f(&mut v.instance, &v.label, true),
            Mode::Probation(p) => {
                f(&mut p.standby.instance, &p.standby.label, false);
                f(&mut p.candidate.instance, &p.candidate.label, true);
            }
            Mode::Disabled { version, .. } => f(&mut version.instance, &version.label, false),
            // Suspended: no instance, so no windows.
            Mode::Suspended(_) | Mode::Moving => {}
        }
    }

    /// The component's name.
    pub fn name(&self) -> &str {
        &self.core.name
    }

    /// Whether the app is suspended (`ctl suspend`): no instance, its state
    /// saved.
    pub fn suspended(&self) -> bool {
        matches!(self.mode, Mode::Suspended(_))
    }

    /// Brings a suspended app back, as `ctl resume` from the kernel does, so an
    /// event for its kept window (a close, an agent's action) reaches it
    /// instead of being dropped. The resume's reply, or `None` when the app was
    /// not suspended.
    pub fn wake(&mut self) -> Result<Option<Vec<u8>>, Exit> {
        if !self.suspended() {
            return Ok(None);
        }
        let request = alloc::format!("{}resume", kabi::live::CONTROL_PREFIX);
        self.handle(0, request.as_bytes()).map(Some)
    }

    /// How long to wait for a request before [`Host::tick`]; 0 is forever.
    pub fn timeout_ms(&self) -> u64 {
        match self.mode {
            Mode::Probation(_) => self.core.settings.health_ms,
            _ => 0,
        }
    }

    fn serve(
        &mut self,
        mut version: Version<E::Instance>,
        request: &[u8],
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        let inputs = self.core.inputs();
        let storage = self.core.storage.clone();
        let result = self
            .core
            .run(&mut version, request, inputs, &storage)
            .map_err(Stop::Fault)
            .and_then(|(reply, txn)| {
                let undo = self.core.commit(&mut version, &txn)?;
                Ok(self.core.performed(reply, &txn, inputs, undo))
            });
        match result {
            Ok(reply) => {
                self.core.remember(request);
                Ok((Mode::Serving(version), reply))
            }
            Err(Stop::Exit(exit)) => Err(exit),
            Err(Stop::Fault(fault)) => {
                self.core.reload_after(&mut version, &fault, false)?;
                let reply = format!(
                    "err {} failed: {}: {}",
                    self.core.name,
                    fault.reason.name(),
                    fault.detail
                );
                Ok((Mode::Serving(version), text(reply)))
            }
        }
    }
}
