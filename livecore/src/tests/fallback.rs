//! A fresh start falls back when the configured version cannot start (M9d: a
//! boot manifest's good version that will not load boots 2 instead).

use super::fake::Bug;
use super::{V1, broken, call, host_with, kernel, ledger};
use crate::host::{EXIT_NO_VERSION, Host};
use crate::settings::Settings;
use alloc::boxed::Box;

const GOOD: &str = "c-0123456789ab";

#[test]
fn a_version_that_will_not_load_falls_back_and_says_why() {
    let kernel = kernel(&[V1, broken(GOOD, Bug::Link)]);
    let settings = "version=c-0123456789ab fallback=1 probation_calls=3 probation_ms=0";
    let mut host = host_with(&kernel, 10, settings);
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let ledger = ledger(&kernel);
    assert_eq!(ledger.active(), Some("1"));
    assert_eq!(ledger.lineage.iter().collect::<alloc::vec::Vec<_>>(), ["1"]);
    let log = kernel.log.borrow();
    assert!(
        log.iter().any(
            |line| line.starts_with("cannot load version c-0123456789ab: ")
                && line.ends_with("; starting 1 instead")
        ),
        "{log:?}"
    );
    // A later host restores the version that started, not the configured one.
    drop(log);
    let mut again = host_with(&kernel, 11, settings);
    assert_eq!(call(&mut again, "get"), "ok 0 schema 1");
    assert_eq!(kernel.component.borrow().fetches(GOOD), 1);
}

#[test]
fn a_version_that_will_not_start_falls_back() {
    // It loads, then its export claims the wrong schema.
    let kernel = kernel(&[V1, broken(GOOD, Bug::Export)]);
    let mut host = host_with(&kernel, 10, "version=c-0123456789ab fallback=1");
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let log = kernel.log.borrow();
    assert!(
        log.iter().any(
            |line| line.starts_with("version c-0123456789ab failed to start: ")
                && line.ends_with("; starting 1 instead")
        ),
        "{log:?}"
    );
}

#[test]
fn a_missing_version_falls_back_and_without_a_fallback_the_host_ends() {
    let kernel = kernel(&[V1]);
    let mut host = host_with(&kernel, 10, "version=c-0123456789ab fallback=1");
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let kernel = super::kernel(&[V1, broken(GOOD, Bug::Link)]);
    let env = kernel.env(10, Box::new(super::fake::load));
    let settings = Settings::parse("version=c-0123456789ab").unwrap();
    let exit = Host::start(env, "counter", settings).err().unwrap();
    assert_eq!(exit.code, EXIT_NO_VERSION);
    assert_eq!(ledger(&kernel).active(), None);
}
