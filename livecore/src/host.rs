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

use crate::artifact::{self, Manifest};
use crate::control;
use crate::settings::Settings;
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
    pub blob: &'a [u8],
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
    /// Only while a request moves the host from one mode to another.
    Moving,
}

/// What the host keeps besides its mode.
pub(crate) struct Core<E> {
    pub env: E,
    pub name: String,
    pub settings: Settings,
    /// The active version's state, as the kernel holds it.
    pub checkpoint: Vec<u8>,
    restarts: Vec<u64>,
    /// The last [`REPLAY`] requests served, oldest first.
    pub recent: VecDeque<Vec<u8>>,
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
            restarts: Vec::new(),
            recent: VecDeque::new(),
        }
    }

    /// Reports an event; a refusal ends the host.
    pub fn report(&mut self, event: &Report<'_>) -> Result<(), Exit> {
        self.env.report(event).map_err(|error| {
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
        let schema = instance.schema()?;
        Ok(Version {
            label: label.to_string(),
            schema,
            manifest,
            artifact,
            instance,
        })
    }

    /// Exports `version`'s state and sends it as a checkpoint if it changed.
    pub fn checkpoint(&mut self, version: &mut Version<E::Instance>) -> Result<(), Stop> {
        let blob = export_own(version)?;
        if blob != self.checkpoint {
            self.report(&Report {
                schema: u64::from(version.schema),
                blob: &blob,
                ..Report::new(kind::CHECKPOINT)
            })?;
            self.checkpoint = blob;
        }
        Ok(())
    }

    /// A new instance of `version` holding the last checkpoint.
    pub fn reload(
        &mut self,
        version: &Version<E::Instance>,
        quiet: bool,
    ) -> Result<E::Instance, Fault> {
        let mut instance = self.env.load(&version.artifact, quiet)?;
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
            None => match mode {
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
        let result = version
            .instance
            .handle(request)
            .map_err(Stop::Fault)
            .and_then(|reply| self.core.checkpoint(&mut version).map(|()| reply));
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
