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
use crate::control::{self, Control};
use crate::settings::Settings;
use crate::{Fault, Instance, Reason};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{flag, kind, phase};

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

fn exit(code: i32, why: impl Into<String>) -> Exit {
    Exit {
        code,
        why: why.into(),
    }
}

impl<E: Env> Core<E> {
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

    /// Fetches, checks and loads a version. `interface` is the active version's,
    /// which the candidate must keep.
    pub fn stage(
        &mut self,
        label: &str,
        artifact: Vec<u8>,
        interface: Option<&str>,
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
        if let Some(interface) = interface
            && manifest.interface != interface
        {
            return Err(Fault::new(
                Reason::Manifest,
                format!(
                    "interface {} differs from {interface}; that needs a restart",
                    manifest.interface
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

impl<E: Env> Host<E> {
    /// Starts from the kernel's ledger: finishes a swap a dead host left open,
    /// restores the active version, or starts the configured one fresh.
    pub fn start(env: E, name: &str, settings: Settings) -> Result<Self, Exit> {
        let mut core = Core {
            env,
            name: name.to_string(),
            settings,
            checkpoint: Vec::new(),
            restarts: Vec::new(),
            recent: VecDeque::new(),
        };
        let mut restored = core
            .env
            .restore()
            .map_err(|error| exit(EXIT_REFUSED, format!("cannot read the ledger: {error:?}")))?;
        if restored.phase == phase::SWAPPING {
            core.report(&Report {
                reason: Some(Reason::HostRestart),
                label: &restored.candidate,
                ..Report::new(kind::ROLLBACK)
            })?;
            restored.phase = phase::STEADY;
        }
        let label = match restored.phase {
            phase::EMPTY => core.settings.version.clone(),
            _ => restored.active.clone(),
        };
        let artifact = core.env.fetch(&label).map_err(|error| {
            exit(
                EXIT_NO_VERSION,
                format!("cannot fetch version {label}: {error:?}"),
            )
        })?;
        let mut version = core.stage(&label, artifact, None, false).map_err(|fault| {
            exit(
                EXIT_NO_VERSION,
                format!("cannot load version {label}: {}", fault.detail),
            )
        })?;
        let mode = if restored.phase == phase::EMPTY {
            Self::fresh(&mut core, version)?
        } else if restored.disabled {
            Mode::Disabled {
                version,
                why: String::from("it was disabled before this host started"),
            }
        } else {
            core.checkpoint = restored.checkpoint;
            match restore_into(&mut version, &core.checkpoint, restored.schema) {
                Ok(blob) => {
                    core.report(&Report {
                        label: &label,
                        schema: u64::from(version.schema),
                        blob: &blob,
                        ..Report::new(kind::RESTORED)
                    })?;
                    core.checkpoint = blob;
                    Mode::Serving(version)
                }
                Err(fault) => {
                    core.report(&Report {
                        reason: Some(fault.reason),
                        detail: &fault.detail,
                        ..Report::new(kind::DISABLED)
                    })?;
                    Mode::Disabled {
                        version,
                        why: format!("{}: {}", fault.reason.name(), fault.detail),
                    }
                }
            }
        };
        Ok(Self { core, mode })
    }

    /// Starts `version` empty.
    fn fresh(
        core: &mut Core<E>,
        mut version: Version<E::Instance>,
    ) -> Result<Mode<E::Instance>, Exit> {
        let fail = |why: String| exit(EXIT_NO_VERSION, why);
        match version.instance.init() {
            Ok(0) => {}
            Ok(code) => {
                return Err(fail(format!(
                    "version {} failed to start ({code})",
                    version.label
                )));
            }
            Err(fault) => {
                return Err(fail(format!(
                    "version {} failed to start: {}",
                    version.label, fault.detail
                )));
            }
        }
        let blob = export_own(&mut version).map_err(|fault| fail(fault.detail))?;
        core.report(&Report {
            label: &version.label,
            schema: u64::from(version.schema),
            blob: &blob,
            ..Report::new(kind::RESTORED)
        })?;
        core.checkpoint = blob;
        Ok(Mode::Serving(version))
    }

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

    fn control(
        &mut self,
        mode: Mode<E::Instance>,
        control: Control,
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        let name = self.core.name.clone();
        Ok(match (control, mode) {
            (Control::Status, mode) => {
                let status = status(&name, &mode, &self.core.settings);
                (mode, text(status))
            }
            (Control::Source { file }, mode) => {
                let reply = match active(&mode) {
                    Some(version) => source(version, file.as_deref()),
                    None => String::from("err no active version"),
                };
                (mode, text(reply))
            }
            (Control::Swap { label, force }, Mode::Serving(version)) => {
                crate::swap::swap(&mut self.core, version, &label, force)?
            }
            (Control::Swap { .. } | Control::Try { .. }, mode @ Mode::Probation(_)) => {
                (mode, text("err busy: a version is on probation"))
            }
            (Control::Try { label }, Mode::Serving(version)) => {
                let reply = crate::try_run::rehearse(&mut self.core, &version, &label);
                (Mode::Serving(version), text(reply))
            }
            (Control::Rollback, Mode::Probation(probation)) => crate::probation::roll_back(
                &mut self.core,
                *probation,
                &Fault::new(Reason::Manual, ""),
                0,
            )
            .map(|(mode, label)| (mode, text(format!("ok rolled back {label}"))))?,
            (Control::Rollback, mode) => (mode, text("err nothing is on probation")),
            (Control::Reset, Mode::Disabled { version, .. }) => self.reset(version)?,
            (Control::Reset, mode) => (mode, text(format!("err {name} is not disabled"))),
            (_, mode @ Mode::Disabled { .. }) => (
                mode,
                text(format!("err {name} is disabled; ctl reset starts fresh")),
            ),
            (_, Mode::Moving) => unreachable!("a request found the host between modes"),
        })
    }

    /// `ctl reset`: forget the state and start the active version empty.
    fn reset(
        &mut self,
        version: Version<E::Instance>,
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        self.core.report(&Report::new(kind::RESET))?;
        let label = version.label.clone();
        let fresh = self
            .core
            .stage(&label, version.artifact, None, false)
            .map_err(|fault| exit(EXIT_NO_VERSION, fault.detail))?;
        let mode = Self::fresh(&mut self.core, fresh)?;
        Ok((
            mode,
            text(format!(
                "ok {} started fresh on version {label}",
                self.core.name
            )),
        ))
    }
}

/// Imports `checkpoint` into `version` and re-exports it in the version's schema.
fn restore_into<I: Instance>(
    version: &mut Version<I>,
    checkpoint: &[u8],
    schema: u64,
) -> Result<Vec<u8>, Fault> {
    let from =
        u32::try_from(schema).map_err(|_| Fault::new(Reason::Migrate, "schema out of range"))?;
    match version.instance.import(checkpoint, from)? {
        0 => export_own(version),
        code => Err(Fault::new(
            Reason::Migrate,
            format!("version {} refused the checkpoint ({code})", version.label),
        )),
    }
}

fn active<I>(mode: &Mode<I>) -> Option<&Version<I>> {
    match mode {
        Mode::Serving(version) | Mode::Disabled { version, .. } => Some(version),
        Mode::Probation(probation) => Some(&probation.standby),
        Mode::Moving => None,
    }
}

fn status<I>(name: &str, mode: &Mode<I>, settings: &Settings) -> String {
    match mode {
        Mode::Serving(version) => format!("ok {name} {} schema {}", version.label, version.schema),
        Mode::Probation(p) => format!(
            "ok {name} {} schema {}; probation {} schema {}: {} of {} calls",
            p.standby.label,
            p.standby.schema,
            p.candidate.label,
            p.candidate.schema,
            p.served,
            settings.probation_calls
        ),
        Mode::Disabled { version, why } => format!("ok {name} {} disabled ({why})", version.label),
        Mode::Moving => String::from("err busy"),
    }
}

/// `ctl source`: the files and sizes, or one file cut to fit a reply.
fn source<I>(version: &Version<I>, file: Option<&str>) -> String {
    let Ok(parsed) = artifact::parse(&version.artifact) else {
        return String::from("err the artifact has no readable source");
    };
    let Ok(files) = artifact::source_files(parsed.source) else {
        return String::from("err the source bundle is damaged");
    };
    let limit = kabi::MAX_MESSAGE - 64;
    match file {
        None => {
            let mut list = format!(
                "ok {} {} ({}):",
                version.manifest.component,
                version.label,
                version.manifest.build.get(..12).unwrap_or("")
            );
            for (path, bytes) in files {
                list.push_str(&format!("\n{path} {}", bytes.len()));
            }
            list
        }
        Some(wanted) => match files.iter().find(|(path, _)| *path == wanted) {
            Some((_, bytes)) => {
                let text = String::from_utf8_lossy(bytes);
                let cut = text.floor_char_boundary(limit);
                format!("ok {}", &text[..cut])
            }
            None => format!("err no file {wanted} in version {}", version.label),
        },
    }
}
