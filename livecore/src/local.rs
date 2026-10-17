//! A host run in-process against a [`Component`] standing in for the kernel: the
//! same store and ledger code the kernel runs, without system calls. The tests use
//! it with scripted instances; xtask uses it with real versions on wasmi to run the
//! exit scenario before booting QEMU.

use crate::host::{Env, Report, Restored};
use crate::ledger::Event;
use crate::store::Component;
use crate::{Fault, Instance};
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use kabi::Error;
use kabi::live::{MAX_DETAIL, MAX_LABEL, Restore};

/// Builds an instance from a module; the flag says whether it is quiet.
pub type Loader<I> = Box<dyn FnMut(&[u8], bool) -> Result<I, Fault>>;

/// The kernel side, shared by the hosts that run one after another.
#[derive(Clone)]
pub struct Kernel {
    pub component: Rc<RefCell<Component>>,
    pub clock: Rc<Cell<u64>>,
    /// The kernel's log lines and the hosts' own.
    pub log: Rc<RefCell<Vec<String>>>,
}

impl Kernel {
    pub fn new(component: Component) -> Self {
        Self {
            component: Rc::new(RefCell::new(component)),
            clock: Rc::new(Cell::new(0)),
            log: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// An environment for a host running as process `pid`.
    pub fn env<I>(&self, pid: u64, loader: Loader<I>) -> LocalEnv<I> {
        LocalEnv {
            kernel: self.clone(),
            pid,
            loader,
        }
    }
}

pub struct LocalEnv<I> {
    pub kernel: Kernel,
    pub pid: u64,
    loader: Loader<I>,
}

/// An event as the kernel would copy it in, with the same size checks.
pub fn event_of(report: &Report<'_>) -> Result<Event, Error> {
    if report.label.len() > MAX_LABEL
        || report.detail.len() > MAX_DETAIL
        || report.blob.len() > kabi::live::MAX_CHECKPOINT
    {
        return Err(Error::TooBig);
    }
    Ok(Event {
        kind: report.kind,
        reason: report.reason.map_or(0, |reason| reason as u64),
        label: report.label.to_string(),
        schema: report.schema,
        served: report.served,
        regressions: report.regressions,
        flags: report.flags,
        blob: report.blob.to_vec(),
        detail: report.detail.to_string(),
    })
}

/// The ledger as [`Restore`] and the checkpoint, as the host reads them.
pub fn restored_of(restore: &Restore, checkpoint: &[u8]) -> Restored {
    Restored {
        name: restore.name().to_string(),
        phase: restore.phase,
        schema: restore.schema,
        disabled: restore.flags & Restore::DISABLED != 0,
        active: restore.active().to_string(),
        candidate: restore.candidate().to_string(),
        checkpoint: checkpoint.to_vec(),
    }
}

impl<I: Instance> Env for LocalEnv<I> {
    type Instance = I;

    fn fetch(&mut self, label: &str) -> Result<Vec<u8>, Error> {
        let mut component = self.kernel.component.borrow_mut();
        let (bytes, _) = component.fetch(self.pid, label, usize::MAX)?;
        Ok(bytes.to_vec())
    }

    fn report(&mut self, report: &Report<'_>) -> Result<(), Error> {
        let event = event_of(report)?;
        let mut component = self.kernel.component.borrow_mut();
        let line = component.event(self.pid, &event)?;
        if let Some(line) = line {
            let name = component.name.clone();
            self.kernel
                .log
                .borrow_mut()
                .push(format!("live: {name} {line}"));
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<Restored, Error> {
        let component = self.kernel.component.borrow();
        Ok(restored_of(&component.restore(), component.checkpoint()))
    }

    fn load(&mut self, module: &[u8], quiet: bool) -> Result<I, Fault> {
        (self.loader)(module, quiet)
    }

    fn now_ms(&self) -> u64 {
        self.kernel.clock.get()
    }

    fn log(&mut self, text: &str) {
        self.kernel.log.borrow_mut().push(text.to_string());
    }
}
