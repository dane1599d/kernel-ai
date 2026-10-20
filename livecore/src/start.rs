//! Starting the component host from the kernel's ledger: finish a swap a dead host
//! left open, restore the active version with its checkpoint, or start the
//! configured version fresh, or its fallback when that one cannot start (M9d: a
//! boot manifest's good version falls back to 2).

use crate::host::{
    Core, EXIT_NO_VERSION, EXIT_REFUSED, Env, Exit, Host, Mode, Report, Version, exit, export_own,
};
use crate::settings::Settings;
use crate::{Fault, Instance, Reason};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{kind, phase};

impl<E: Env> Host<E> {
    /// Starts from the kernel's ledger: finishes a swap a dead host left open,
    /// restores the active version, or starts the configured one fresh.
    pub fn start(env: E, name: &str, settings: Settings) -> Result<Self, Exit> {
        let mut core = Core::new(env, name, settings);
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
        if restored.phase == phase::EMPTY {
            let mode = Self::first(&mut core)?;
            return Ok(Self { core, mode });
        }
        let label = restored.active.clone();
        let mut version = load(&mut core, &label)?;
        let mode = if restored.disabled {
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

    /// A fresh start on the configured version, or on the fallback when that one
    /// cannot be fetched, staged or started; why goes to the log.
    fn first(core: &mut Core<E>) -> Result<Mode<E::Instance>, Exit> {
        let label = core.settings.version.clone();
        let failed = match load(core, &label).and_then(|version| Self::fresh(core, version)) {
            Ok(mode) => return Ok(mode),
            Err(failed) if failed.code == EXIT_NO_VERSION => failed,
            Err(refused) => return Err(refused),
        };
        let Some(fallback) = core.settings.fallback.clone().filter(|f| *f != label) else {
            return Err(failed);
        };
        core.env
            .log(&format!("{}; starting {fallback} instead", failed.why));
        load(core, &fallback).and_then(|version| Self::fresh(core, version))
    }

    /// Starts `version` empty.
    pub(crate) fn fresh(
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
        let blob = export_own(&mut version).map_err(|fault| {
            fail(format!(
                "version {} failed to start: {}",
                version.label, fault.detail
            ))
        })?;
        core.report(&Report {
            label: &version.label,
            schema: u64::from(version.schema),
            blob: &blob,
            ..Report::new(kind::RESTORED)
        })?;
        core.checkpoint = blob;
        Ok(Mode::Serving(version))
    }
}

/// Fetches and stages version `label` to start.
fn load<E: Env>(core: &mut Core<E>, label: &str) -> Result<Version<E::Instance>, Exit> {
    let artifact = core.env.fetch(label).map_err(|error| {
        exit(
            EXIT_NO_VERSION,
            format!("cannot fetch version {label}: {error:?}"),
        )
    })?;
    core.stage(label, artifact, None, false).map_err(|fault| {
        exit(
            EXIT_NO_VERSION,
            format!("cannot load version {label}: {}", fault.detail),
        )
    })
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
