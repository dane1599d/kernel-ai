//! The online repository end to end (P5 task 8's host half): a test-signed
//! repository served by xtask's HTTPS mock as GitHub serves
//! `kernel-ai-updates`, its metadata on the Pages host and each `.kpkg` as a
//! release asset behind the releases host's redirect, and `kaipkg::online`'s
//! client over kainet's TLS, with the store on a `MemVolume`, as `pkg` will run
//! it once it holds TCP. What the mirror refuses, this refuses too, and it
//! reaches no host outside the repository's.

use super::Repository;
use super::mirrors::big;
use super::tests::{NOW, hello, keys, notes};
use super::tuf;
use crate::mock_https::{self, ASSETS, Content, Fault, Mock, RELEASES, REPO};
use kainet::client::{self, Wall};
use kainet::http::{Dial, HttpError, TlsConnect};
use kainet::tls::Transport;
use kaipkg::generation::{Generation, Time};
use kaipkg::memvol::MemVolume;
use kaipkg::mirror::{self, Listing, TUF_DIR, TufCache};
use kaipkg::online::{Hosts, Online};
use kaipkg::resolve::Want;
use kaipkg::store::{Ctx, Error, Ledger, Refused, Store, Volume};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

/// 2030-01-01 for TLS, inside the mock's good certificates; the TUF metadata
/// keeps its own time, [`NOW`].
fn at_2030() -> Option<u64> {
    Some(1_893_456_000)
}

/// One request's budget.
const REQUEST_NS: u64 = 30_000_000_000;

struct Tcp(TcpStream);

impl Transport for Tcp {
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        self.0.write_all(bytes).map_err(|_| "send failed")
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        self.0.read(buf).map_err(|_| "recv failed")
    }
}

struct Local(u16);

impl Dial for Local {
    type Transport = Tcp;

    fn dial(&mut self, _: &str, _: u16) -> Result<Tcp, HttpError> {
        let stream = TcpStream::connect(("127.0.0.1", self.0))
            .map_err(|_| HttpError::Connect("the mock refused"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|_| HttpError::Connect("no timeout"))?;
        Ok(Tcp(stream))
    }
}

/// The mock's hosts laid out as GitHub's.
fn hosts() -> Hosts {
    Hosts {
        metadata: format!("https://{REPO}/metadata/"),
        releases: format!("https://{RELEASES}/dane1599d/kernel-ai-updates/releases/download/"),
        assets: ASSETS.into(),
    }
}

/// `repo` as GitHub would serve it: metadata on Pages, each target as the
/// asset `<name>-<version>.kpkg` of release `pkg-<name>-<version>`.
fn served(repo: &Repository) -> Mock {
    let mut content = Content::new();
    for (path, bytes) in &repo.files {
        if let Some(name) = path.strip_prefix("metadata/") {
            content = content.file(REPO, &format!("/metadata/{name}"), bytes.clone());
        } else if let Some(file) = path.strip_prefix("targets/") {
            let (_, asset) = file.split_once('.').expect("a hash prefix");
            let stem = asset.strip_suffix(".kpkg").expect("a .kpkg");
            content = content.release_asset(&format!("pkg-{stem}"), asset, bytes.clone());
        }
    }
    mock_https::start(content).unwrap()
}

fn online(mock: &Mock) -> Online<'static, TlsConnect<Local>> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mock.ca().to_vec())).unwrap();
    let wall = Wall {
        now_s: at_2030,
        floor_s: client::FLOOR_S,
    };
    let config = client::config(mock_https::provider(), roots, Arc::new(wall)).unwrap();
    let connect = TlsConnect {
        config,
        dial: Local(mock.port),
    };
    Online::new(hosts(), connect, &|| 0, REQUEST_NS)
}

fn file<'a>(repo: &'a Repository, path: &str) -> &'a [u8] {
    let found = repo.files.iter().find(|(p, _)| p == path);
    &found.unwrap_or_else(|| panic!("no {path}")).1
}

/// A device that trusts `repo`'s root.
fn device(repo: &Repository) -> MemVolume {
    let mut vol = MemVolume::default();
    vol.write(
        &format!("{TUF_DIR}/root.json"),
        file(repo, "metadata/1.root.json"),
    )
    .unwrap();
    vol
}

fn listing(vol: &mut MemVolume, mock: &Mock, now: Time) -> Result<Listing, String> {
    mirror::index(&mut online(mock), &mut TufCache { vol }, now.0)
}

struct NoLedger;

impl Ledger for NoLedger {
    fn switched(&mut self, _: &mut dyn Volume, _: u64, _: &Generation) -> Result<(), String> {
        Ok(())
    }

    fn collected(&mut self, _: &mut dyn Volume, _: &[u64], _: usize) -> Result<(), String> {
        Ok(())
    }
}

fn install(vol: &mut MemVolume, mock: &Mock, name: &str) -> Result<u64, Error> {
    let entries = listing(vol, mock, NOW).unwrap().entries;
    let mut ledger = NoLedger;
    let mut cx = Ctx {
        vol,
        ledger: &mut ledger,
        now: NOW,
        action: 1,
    };
    let store = Store::default();
    store.setup(&mut cx, Vec::new()).unwrap();
    let want = [Want { name, range: None }];
    store.install(
        &mut cx,
        &mut online(mock),
        &entries,
        kaipkg::PROVIDED,
        &want,
        false,
    )
}

const BIG: &str = "/assets/pkg-big-1.0.0/big-1.0.0.kpkg";

#[test]
fn packages_list_from_pages_and_install_from_releases_through_the_redirect() {
    let repo = tuf::build(&[notes(), hello(), big()], &keys(1), NOW).unwrap();
    let mock = served(&repo);
    let mut vol = device(&repo);
    let found = listing(&mut vol, &mock, NOW).unwrap();
    assert!(found.refused.is_empty(), "{:?}", found.refused);
    let names: Vec<&str> = found.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["big", "hello", "notes"]);
    assert_eq!(install(&mut vol, &mock, "big"), Ok(2));
    let served = mock.served();
    assert!(
        served
            .iter()
            .any(|s| s.host == REPO && s.path == "/metadata/timestamp.json")
    );
    let pieces: Vec<_> = served
        .iter()
        .filter(|s| s.host == ASSETS && s.path == BIG)
        .collect();
    assert_eq!(pieces.len(), 2, "300 KiB comes in two 192 KiB pieces");
    assert!(pieces.iter().all(|s| s.status == 206));
    let redirected = served
        .iter()
        .filter(|s| s.host == RELEASES && s.status == 302)
        .count();
    assert_eq!(redirected, 2, "each piece by the releases host's redirect");
}

#[test]
fn a_flipped_byte_in_a_release_asset_is_refused_by_name() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let mock = served(&repo);
    mock.set_fault(ASSETS, BIG, Fault::Flip(200_000));
    let mut vol = device(&repo);
    match install(&mut vol, &mock, "big") {
        Err(Error::Refused(why)) => assert_eq!(why, Refused::BadHash("big".into())),
        other => panic!("not refused: {other:?}"),
    }
    assert!(!vol.files().keys().any(|p| p.contains(".partial")));
}

#[test]
fn a_redirect_off_the_repository_is_never_followed() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let mock = served(&repo);
    mock.set_fault(ASSETS, BIG, Fault::RedirectOff);
    let mut vol = device(&repo);
    match install(&mut vol, &mock, "big") {
        Err(Error::Store(why)) => assert!(why.contains("Refused(\"elsewhere.kai.test\")"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(mock.served().iter().all(|s| s.host != mock_https::OFF));
}

#[test]
fn a_bad_certificate_or_an_expired_timestamp_lists_nothing() {
    let repo = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let mock = served(&repo);
    let late = Time(NOW.0 + 7 * 86_400);
    let err = listing(&mut device(&repo), &mock, late).unwrap_err();
    assert!(err.starts_with("expired: timestamp"), "{err}");
    mock.set_fault(REPO, "*", Fault::Expired);
    let err = listing(&mut device(&repo), &mock, NOW).unwrap_err();
    assert!(err.contains("Expired"), "{err}");
}

#[test]
fn roles_signed_by_keys_the_device_does_not_trust_are_refused() {
    let trusted = tuf::build(&[big()], &keys(1), NOW).unwrap();
    let forged = tuf::build(&[big()], &keys(2), NOW).unwrap();
    let mock = served(&forged);
    let err = listing(&mut device(&trusted), &mock, NOW).unwrap_err();
    assert!(
        err.starts_with("below the signature threshold: timestamp"),
        "{err}"
    );
}

#[test]
fn githubs_hosts_are_the_fixed_destination() {
    let github = Hosts::github();
    for host in [
        "dane1599d.github.io",
        "github.com",
        "release-assets.githubusercontent.com",
    ] {
        assert!(github.allows(host, 443), "{host}");
        assert!(!github.allows(host, 80), "{host}:80");
    }
    for host in [
        "evil.github.io",
        "objects.githubusercontent.com",
        "huggingface.co",
    ] {
        assert!(!github.allows(host, 443), "{host}");
    }
}
