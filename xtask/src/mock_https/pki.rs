//! The mock's certificates: a test CA (`kainet::testca`), and for every host
//! a good certificate, an expired one, one for another name and one from a CA
//! no client trusts. The certificate each handshake gets follows SNI and the
//! host's certificate fault, so a case changes a host's certificate between
//! connections without restarting anything.

use super::{Fault, Faults, HOSTS, fault_for};
use kainet::testca::{Ca, Days, Key, KeyKind};
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert, ServerConfig};
use rustls::sign::CertifiedKey;
use rustls::time_provider::TimeProvider;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// What a good certificate's validity is, and an expired one's.
const VALID: Days = ((2025, 1, 1), (2045, 1, 1));
const EXPIRED: Days = ((2020, 1, 1), (2021, 1, 1));
/// The name a wrong-name certificate holds.
const WRONG: &str = "wrong.kai.test";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Variant {
    Good,
    Expired,
    WrongName,
    UnknownCa,
}

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::getrandom(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

/// kainet's provider, made once for the whole of xtask.
pub fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(kainet::provider::provider(&RANDOM)))
        .clone()
}

#[derive(Debug)]
pub struct Wall;

impl TimeProvider for Wall {
    fn current_time(&self) -> Option<UnixTime> {
        let since = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?;
        Some(UnixTime::since_unix_epoch(since))
    }
}

pub struct Pki {
    ca: Vec<u8>,
    certs: BTreeMap<(&'static str, Variant), Arc<CertifiedKey>>,
}

impl std::fmt::Debug for Pki {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pki")
            .field("certs", &self.certs.len())
            .finish()
    }
}

fn seed(n: u8) -> [u8; 32] {
    let mut seed = [0x4du8; 32];
    seed[0] = n;
    seed
}

/// The one PKI of this xtask run, made on first use: every mock started in the
/// run shares its CA, so a boot case can carry the CA it trusts in the image
/// before its own mock exists (the update cases build the image the mock serves,
/// with the CA already in it).
pub fn shared() -> Result<Arc<Pki>, String> {
    static SHARED: OnceLock<Result<Arc<Pki>, String>> = OnceLock::new();
    SHARED.get_or_init(|| Pki::new().map(Arc::new)).clone()
}

impl Pki {
    pub fn new() -> Result<Self, String> {
        let key = |n| Key::new(KeyKind::P256, seed(n));
        let ca = Ca::new("kai mock CA", key(1)?, VALID).map_err(|e| e.to_string())?;
        let unknown = Ca::new("kai unknown CA", key(2)?, VALID).map_err(|e| e.to_string())?;
        let server = key(3)?;
        let pkcs8 = PrivatePkcs8KeyDer::from(server.pkcs8()?);
        let signing = provider()
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(pkcs8))
            .map_err(|e| e.to_string())?;
        let mut certs = BTreeMap::new();
        for &host in HOSTS {
            for (variant, issuer, names, valid) in [
                (Variant::Good, &ca, host, VALID),
                (Variant::Expired, &ca, host, EXPIRED),
                (Variant::WrongName, &ca, WRONG, VALID),
                (Variant::UnknownCa, &unknown, host, VALID),
            ] {
                let der = issuer
                    .issue(&[names], &server, valid)
                    .map_err(|e| e.to_string())?;
                let chain = vec![CertificateDer::from(der)];
                certs.insert(
                    (host, variant),
                    Arc::new(CertifiedKey::new(chain, Arc::clone(&signing))),
                );
            }
        }
        Ok(Self {
            ca: ca.der().to_vec(),
            certs,
        })
    }

    pub fn ca_der(&self) -> &[u8] {
        &self.ca
    }

    /// One connection's configuration: TLS 1.3, certificates by SNI and
    /// fault. The name the client sent lands in `sni`, as rustls's unbuffered
    /// server does not give it back.
    pub fn server_config(
        self: &Arc<Self>,
        faults: Faults,
        sni: Sni,
    ) -> Result<Arc<ServerConfig>, String> {
        let resolver = Resolver {
            pki: Arc::clone(self),
            faults,
            sni,
        };
        let config = ServerConfig::builder_with_details(provider(), Arc::new(Wall))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        Ok(Arc::new(config))
    }
}

/// Where a connection's resolver leaves the name its client sent.
pub type Sni = Arc<Mutex<Option<&'static str>>>;

#[derive(Debug)]
struct Resolver {
    pki: Arc<Pki>,
    faults: Faults,
    sni: Sni,
}

impl ResolvesServerCert for Resolver {
    /// No SNI, or a name the mock does not serve, ends the handshake.
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?;
        let host = HOSTS.iter().find(|h| **h == name)?;
        *super::lock(&self.sni) = Some(host);
        let variant = match fault_for(&self.faults, host, "*") {
            Some(Fault::Expired) => Variant::Expired,
            Some(Fault::WrongName) => Variant::WrongName,
            Some(Fault::UnknownCa) => Variant::UnknownCa,
            _ => Variant::Good,
        };
        self.pki.certs.get(&(*host, variant)).cloned()
    }
}
