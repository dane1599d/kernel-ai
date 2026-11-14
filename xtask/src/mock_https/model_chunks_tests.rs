//! Lane U's half of P5 task 9 over the HTTPS mock: the `models` role as xtask
//! signs it, with each model's `kai-chunks 1` list beside it as
//! `<sha256>.chunks`, read from the repository host and verified by `kaituf`
//! the way the OS verifies it; then the model downloaded by `kainet` against
//! that list. A list whose bytes changed after signing, and a list that names
//! another model, are refused before any of the model is fetched.

use super::*;
use crate::pkgrepo::{models, tuf};
use kainet::chunks::{CHUNK, Chunks};
use kainet::client::{self, Wall};
use kainet::download::{Answer, DownloadError, Partial, download};
use kainet::http::{Dial, Fetch, HttpError, Limits, Method, Range, TlsConnect, Url};
use kaipkg::generation::Time;
use kaituf::trusted::{ROOT, TrustedSet};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

/// 2030-01-01: inside the mock's certificates, and the repository is signed
/// then, so its roles are fresh.
const NOW: u64 = 1_893_456_000;
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

fn at_2030() -> Option<u64> {
    Some(NOW)
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

fn connect(mock: &Mock) -> TlsConnect<Local> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mock.ca().to_vec())).unwrap();
    let wall = Wall {
        now_s: at_2030,
        floor_s: client::FLOOR_S,
    };
    TlsConnect {
        config: client::config(pki::provider(), roots, Arc::new(wall)).unwrap(),
        dial: Local(mock.port),
    }
}

fn on_443(host: &str, port: u16) -> bool {
    port == 443 && (host == REPO || host == MODELS)
}

/// The whole of `path` on `host`, as `updated` reads the repository.
fn get(mock: &Mock, host: &str, path: &str) -> Vec<u8> {
    let zero = || 0;
    let fetch = Fetch {
        limits: Limits::new(1 << 24, u64::MAX),
        allow: &on_443,
        clock: &zero,
    };
    let url = Url::parse(&format!("https://{host}{path}")).unwrap();
    let mut body = Vec::new();
    let got = fetch
        .run(&mut connect(mock), Method::Get, &url, None, &mut |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(got.head.status, 200, "{host}{path}");
    body
}

/// The fixture model, its GGUF's bytes, and the repository xtask signs for it,
/// laid out as the update repository serves it.
struct Signed {
    model: models::Model,
    bytes: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
}

fn signed() -> Signed {
    let model = models::fixture();
    let bytes = crate::disk::Fixture::Gguf.bytes();
    let keys = tuf::Keys::generate().unwrap();
    let repo = tuf::build_with_models(&[], std::slice::from_ref(&model), &keys, Time(NOW)).unwrap();
    Signed {
        model,
        bytes,
        files: repo.files,
    }
}

/// A target's SHA-256, as 32 bytes.
fn sha256_of(target: &kaituf::meta::TargetFile) -> [u8; 32] {
    let (_, sha) = target
        .hashes
        .iter()
        .find(|(name, _)| name == "sha256")
        .unwrap();
    sha.as_slice().try_into().unwrap()
}

/// The `models` role, trusted from the root the repository was signed with,
/// fetched role by role from the repository host.
fn trusted(mock: &Mock) -> TrustedSet {
    let root = get(mock, REPO, "/metadata/1.root.json");
    let mut set = TrustedSet::new(&root, NOW).unwrap();
    set.update_timestamp(&get(mock, REPO, "/metadata/timestamp.json"))
        .unwrap();
    set.update_snapshot(&get(mock, REPO, "/metadata/1.snapshot.json"), false)
        .unwrap();
    set.update_targets(
        &get(mock, REPO, "/metadata/1.targets.json"),
        "targets",
        ROOT,
    )
    .unwrap();
    set.update_targets(
        &get(mock, REPO, "/metadata/1.models.json"),
        "models",
        "targets",
    )
    .unwrap();
    set
}

/// The model's list as the role signs it: fetched by its consistent-snapshot
/// name and held to the role's length and SHA-256, then parsed and held to the
/// model's target. The error says which check refused it.
fn signed_list(mock: &Mock, set: &TrustedSet, gguf: &str) -> Result<Chunks, String> {
    let role = &set
        .targets("models")
        .ok_or("no models role")?
        .signed
        .targets;
    let model = role.get(gguf).ok_or("no model target")?;
    let model_sha = sha256_of(model);
    let name = kaipkg::models::chunks_path(&model_sha);
    let list = role.get(&name).ok_or("no chunk list beside the model")?;
    let list_sha = kaipkg::hex(&sha256_of(list));
    let bytes = get(mock, REPO, &format!("/targets/{list_sha}.{name}"));
    if bytes.len() as u64 != list.length || crate::sha256_hex(&bytes) != list_sha {
        return Err("the list is not the one the role signs".into());
    }
    let chunks = Chunks::parse(&bytes).map_err(|why| why.to_string())?;
    if chunks.size != model.length || chunks.blob != model_sha {
        return Err("the list is for another model".into());
    }
    Ok(chunks)
}

#[derive(Default)]
struct Memory {
    bytes: Vec<u8>,
    done: usize,
    finished: bool,
}

impl Partial for Memory {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        Ok(self.done)
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.done = done;
        Ok(())
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        let end = offset as usize + bytes.len();
        if self.bytes.len() < end {
            self.bytes.resize(end, 0);
        }
        self.bytes[offset as usize..end].copy_from_slice(bytes);
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        let start = offset as usize;
        buf.copy_from_slice(&self.bytes[start..start + buf.len()]);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        self.finished = true;
        Ok(())
    }

    fn size(&mut self) -> Result<u64, DownloadError> {
        Ok(self.bytes.len() as u64)
    }
}

#[test]
fn the_signed_list_beside_a_model_checks_its_download_chunk_by_chunk() {
    let Signed {
        model,
        bytes,
        files,
    } = signed();
    let file = "fixture.gguf";
    let mock = start(Content::new().repository(files).model(
        "kai/fixture",
        REVISION,
        file,
        bytes.clone(),
    ))
    .unwrap();
    let set = trusted(&mock);
    let list = signed_list(&mock, &set, &model.target()).unwrap();
    assert_eq!(list.chunk, CHUNK);
    assert_eq!(list.count() as u64, model.length.div_ceil(CHUNK));
    let zero = || 0;
    let fetch = Fetch {
        limits: Limits::new(CHUNK, u64::MAX),
        allow: &on_443,
        clock: &zero,
    };
    let url = Url::parse(&format!(
        "https://{MODELS}/kai/fixture/resolve/{REVISION}/{file}"
    ))
    .unwrap();
    let mut partial = Memory::default();
    let mut connect = connect(&mock);
    download(&list, &mut partial, &mut |range: Range, sink| {
        let got = fetch.run(&mut connect, Method::Get, &url, Some(range), sink)?;
        Ok(Answer::from(&got))
    })
    .unwrap();
    assert!(partial.finished);
    assert_eq!(partial.bytes, bytes);
}

#[test]
fn a_changed_list_and_a_list_for_another_model_are_refused_before_the_model() {
    let Signed {
        model,
        bytes,
        mut files,
    } = signed();
    // One byte of the served list changes after signing.
    let list = files
        .iter_mut()
        .find(|(path, _)| path.ends_with(".chunks"))
        .unwrap();
    *list.1.last_mut().unwrap() ^= 1;
    let mock = start(Content::new().repository(files.clone()).model(
        "kai/fixture",
        REVISION,
        "fixture.gguf",
        bytes.clone(),
    ))
    .unwrap();
    let set = trusted(&mock);
    let why = signed_list(&mock, &set, &model.target()).unwrap_err();
    assert!(why.contains("not the one the role signs"), "{why}");
    assert!(mock.served().iter().all(|s| s.host != MODELS));

    // A list made for the second fixture, signed as if it were this model's,
    // names another blob, so it never pairs.
    let other = models::fixture_2();
    let mut forged = models::fixture();
    forged.chunks = other.chunks.clone();
    let keys = tuf::Keys::generate().unwrap();
    let repo = tuf::build_with_models(&[], &[forged], &keys, Time(NOW)).unwrap();
    let mock = start(Content::new().repository(repo.files).model(
        "kai/fixture",
        REVISION,
        "fixture.gguf",
        bytes,
    ))
    .unwrap();
    let set = trusted(&mock);
    let why = signed_list(&mock, &set, &model.target()).unwrap_err();
    assert!(why.contains("another model"), "{why}");
    assert!(mock.served().iter().all(|s| s.host != MODELS));
}
