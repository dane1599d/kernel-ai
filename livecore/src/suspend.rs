//! Suspending an app (P3 task 11, the founder's standing instructions): `ctl
//! suspend` saves the serving version's state through `kai:live`'s `save`, as
//! every call's commit does, and drops its instance, so its memory and store go
//! back to the host's heap; `ctl resume`, or the next request, loads the same
//! artifact again and restores the state through `init` and `migrate`. Nothing
//! the app holds is lost: the checkpoint and the storage stay with the host, and
//! the kernel's ledger already has them.

use crate::host::{Core, Env, Exit, Host, Mode, Stop, Version, text};
use crate::manifest::Manifest;
use crate::txn::Txn;
use crate::{Fault, Instance};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A suspended version: everything but its instance.
pub(crate) struct Parked {
    pub label: String,
    pub schema: u32,
    pub manifest: Manifest,
    pub artifact: Vec<u8>,
}

/// A version whose state would not save, still running, and why.
type Unsaved<I> = Box<(Version<I>, Stop)>;

impl<E: Env> Core<E> {
    /// Saves `version`'s state and drops its instance; on a failed save, the
    /// version back, still running.
    fn park(&mut self, mut version: Version<E::Instance>) -> Result<Parked, Unsaved<E::Instance>> {
        if let Err(stop) = self.commit(&mut version, &Txn::default()) {
            return Err(Box::new((version, stop)));
        }
        let Version {
            label,
            schema,
            manifest,
            artifact,
            instance,
        } = version;
        drop(instance);
        Ok(Parked {
            label,
            schema,
            manifest,
            artifact,
        })
    }

    /// A new instance of the parked version holding its checkpoint.
    pub(crate) fn unpark(
        &mut self,
        parked: Parked,
    ) -> Result<Version<E::Instance>, Box<(Parked, Fault)>> {
        let mut instance = match self.env.load(&parked.artifact, false) {
            Ok(instance) => instance,
            Err(fault) => return Err(Box::new((parked, fault))),
        };
        // As at start and reload: an app that keeps its data in storage reads it
        // while it restores.
        instance.view(&self.storage);
        let restored = if self.checkpoint.is_empty() {
            instance.init()
        } else {
            instance.import(&self.checkpoint, parked.schema)
        };
        match restored {
            Ok(0) => Ok(Version {
                label: parked.label,
                schema: parked.schema,
                manifest: parked.manifest,
                artifact: parked.artifact,
                instance,
            }),
            Ok(code) => {
                let why = format!("it refused its own checkpoint ({code})");
                Err(Box::new((parked, Fault::new(crate::Reason::Migrate, why))))
            }
            Err(fault) => Err(Box::new((parked, fault))),
        }
    }
}

impl<E: Env> Host<E> {
    /// `ctl suspend`: only a version that is serving alone.
    pub(crate) fn suspend(
        &mut self,
        mode: Mode<E::Instance>,
    ) -> Result<(Mode<E::Instance>, Vec<u8>), Exit> {
        let name = self.core.name.clone();
        let version = match mode {
            Mode::Serving(version) => version,
            Mode::Suspended(parked) => {
                return Ok((
                    Mode::Suspended(parked),
                    text(format!("err {name} is suspended")),
                ));
            }
            other => {
                let why = format!("err {name} is not serving alone; finish its probation first");
                return Ok((other, text(why)));
            }
        };
        match self.core.park(version).map_err(|e| *e) {
            Ok(parked) => {
                let reply = format!("ok suspended {name} {}", parked.label);
                self.core
                    .env
                    .log(&format!("{name}: suspended version {}", parked.label));
                Ok((Mode::Suspended(parked), text(reply)))
            }
            Err((_, Stop::Exit(exit))) => Err(exit),
            // Its state would not save: it keeps running, nothing lost.
            Err((version, Stop::Fault(fault))) => {
                let why = format!("err {name} cannot save to suspend: {}", fault.detail);
                Ok((Mode::Serving(version), text(why)))
            }
        }
    }

    /// `ctl resume`, or a request that finds the app suspended: the version
    /// again, from its checkpoint. A version that cannot come back stays
    /// suspended and says why.
    pub(crate) fn resume(
        &mut self,
        mode: Mode<E::Instance>,
    ) -> (Mode<E::Instance>, Result<String, String>) {
        let name = self.core.name.clone();
        let Mode::Suspended(parked) = mode else {
            return (mode, Err(format!("err {name} is not suspended")));
        };
        match self.core.unpark(parked).map_err(|e| *e) {
            Ok(version) => {
                let reply = format!("ok resumed {name} {}", version.label);
                self.core
                    .env
                    .log(&format!("{name}: resumed version {}", version.label));
                (Mode::Serving(version), Ok(reply))
            }
            Err((parked, fault)) => {
                let why = format!(
                    "err {name} cannot resume: {}: {}",
                    fault.reason.name(),
                    fault.detail
                );
                (Mode::Suspended(parked), Err(why))
            }
        }
    }
}
