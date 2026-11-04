//! The OS's TLS client configuration (plan p5-networking, task 4): TLS 1.3
//! only, our provider, the roots it is given, no client certificate, and a
//! clock that refuses to be earlier than the build. A clock before the build
//! date makes rustls refuse every certificate by name
//! (`FailedToGetCurrentTime`) rather than check them against a wrong time
//! (§9's risk row).

use alloc::sync::Arc;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::UnixTime;
use rustls::time_provider::TimeProvider;
use rustls::{ClientConfig, RootCertStore};

/// The earliest a correct clock can read: 2026-11-01, before any build of
/// this crate. A build may pass its own, later date.
pub const FLOOR_S: u64 = 1_793_491_200;

/// The wall clock, as seconds since 1970 from `now_s`, refused before
/// `floor_s`.
#[derive(Debug, Clone, Copy)]
pub struct Wall {
    pub now_s: fn() -> Option<u64>,
    pub floor_s: u64,
}

impl TimeProvider for Wall {
    fn current_time(&self) -> Option<UnixTime> {
        let now = (self.now_s)()?;
        (now >= self.floor_s)
            .then(|| UnixTime::since_unix_epoch(core::time::Duration::from_secs(now)))
    }
}

/// A client configuration for `roots`, at the time `time` gives.
pub fn config(
    provider: Arc<CryptoProvider>,
    roots: RootCertStore,
    time: Arc<dyn TimeProvider>,
) -> Result<Arc<ClientConfig>, rustls::Error> {
    let config = ClientConfig::builder_with_details(provider, time)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Mozilla's roots (feature `roots`): webpki-roots, CDLA-Permissive-2.0,
/// updated with the signed OS (P5 question 2).
#[cfg(feature = "roots")]
pub fn mozilla_roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}
