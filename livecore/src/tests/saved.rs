//! An app's state across a reboot: the kernel starts empty, and the host
//! starts from the record comphost saved in the app's folder.

use super::fake::{self, Fake};
use super::{SETTINGS, V1, call, host, kernel, ledger};
use crate::Fault;
use crate::host::{Env, Host, Report, Restored};
use crate::local::LocalEnv;
use crate::saved::{self, Record};
use crate::settings::Settings;
use alloc::boxed::Box;
use alloc::vec::Vec;
use kabi::Error;

/// The kernel's env, with comphost's disk read in on restore.
struct WithDisk {
    inner: LocalEnv<Fake>,
    disk: Option<Record>,
}

impl Env for WithDisk {
    type Instance = Fake;

    fn fetch(&mut self, label: &str) -> Result<Vec<u8>, Error> {
        self.inner.fetch(label)
    }

    fn report(&mut self, report: &Report<'_>) -> Result<(), Error> {
        self.inner.report(report)
    }

    fn restore(&mut self) -> Result<Restored, Error> {
        let mut restored = self.inner.restore()?;
        saved::merge(&mut restored, self.disk.take());
        Ok(restored)
    }

    fn load(&mut self, module: &[u8], quiet: bool) -> Result<Fake, Fault> {
        self.inner.load(module, quiet)
    }

    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    fn log(&mut self, text: &str) {
        self.inner.log(text);
    }
}

/// What comphost would have written by now: the kernel's checkpoint.
fn saved_of(kernel: &crate::local::Kernel) -> Record {
    let ledger = ledger(kernel);
    Record {
        generation: ledger.counters.checkpoints,
        schema: ledger.schema as u32,
        checkpoint: kernel.component.borrow().checkpoint().to_vec(),
    }
}

fn boot(kernel: &crate::local::Kernel, disk: Option<Record>) -> Host<WithDisk> {
    let env = WithDisk {
        inner: kernel.env(20, Box::new(fake::load)),
        disk,
    };
    Host::start(env, "counter", Settings::parse(SETTINGS).unwrap()).unwrap()
}

#[test]
fn after_a_reboot_the_app_starts_from_its_saved_state() {
    let before = kernel(&[V1]);
    let mut first = host(&before);
    call(&mut first, "add");
    call(&mut first, "add");
    let record = saved_of(&before);
    // The reboot: a new kernel with an empty ledger, and the disk's record.
    let after = kernel(&[V1]);
    let mut host = boot(&after, Some(record));
    let reply = alloc::string::String::from_utf8(host.handle(0, b"get").unwrap()).unwrap();
    assert_eq!(reply, "ok 2 schema 1");
    assert_eq!(ledger(&after).active(), Some("1"));
    assert!(
        after
            .log
            .borrow()
            .iter()
            .any(|line| line == "live: counter 1 restored from its saved state")
    );
}

#[test]
fn the_kernel_s_ledger_wins_and_no_record_starts_fresh() {
    let k = kernel(&[V1]);
    let mut first = host(&k);
    call(&mut first, "add");
    // The kernel still holds the app (a host restart, not a reboot): an older
    // record on disk is not used.
    let stale = Record {
        generation: 0,
        schema: 1,
        checkpoint: Vec::new(),
    };
    let mut again = boot(&k, Some(stale));
    let reply = alloc::string::String::from_utf8(again.handle(0, b"get").unwrap()).unwrap();
    assert_eq!(reply, "ok 1 schema 1");
    // Nothing anywhere: a fresh start.
    let empty = kernel(&[V1]);
    let mut fresh = boot(&empty, None);
    let reply = alloc::string::String::from_utf8(fresh.handle(0, b"get").unwrap()).unwrap();
    assert_eq!(reply, "ok 0 schema 1");
}

#[test]
fn a_saved_state_the_version_refuses_disables_it_instead_of_starting_empty() {
    let k = kernel(&[V1]);
    let bad = Record {
        generation: 1,
        schema: 99,
        checkpoint: alloc::vec![0x82, 0x18, 99, 1],
    };
    let mut host = boot(&k, Some(bad));
    let status = alloc::string::String::from_utf8(host.handle(0, b"ctl status").unwrap()).unwrap();
    assert!(status.contains("disabled"), "{status}");
}
