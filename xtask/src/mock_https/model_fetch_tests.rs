//! The host half of P5's `model-download` case (task 9): Models' job, planned
//! by `modelchan::fetch` from the signed `models` role, run by `kainet`'s
//! verified download from the mock's Hugging Face-shaped host. As Hugging
//! Face's do, the resolve URL redirects to a storage host, which serves the
//! bytes; the job reaches those two hosts and no other. The chunk list, the
//! role's sibling target, is held to the role's target before a byte is
//! fetched; the bytes wait in `.partial/` and move into the store only once
//! every chunk and the whole SHA-256 have matched. The mock cuts the
//! connection twice, flips a byte, redirects off the job's hosts, or is given a
//! list for other bytes.

use super::*;
use comp_models::{Signed, Store};
use kainet::chunks::{CHUNK, Chunks};
use kainet::client::{self, Wall};
use kainet::download::{Answer, DownloadError, Partial, Report, SHORT_READ, download};
use kainet::http::{Dial, Fetch, HttpError, Limits, Method, Range, TlsConnect, Url};
use modelchan::fetch::{self, Hosts, Job, MODEL_CHUNK, Machine, Refused};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};

/// 2030-01-01, inside the mock's good certificates.
fn at_2030() -> Option<u64> {
    Some(1_893_456_000)
}

const HF_REPO: &str = "kernel-ai/fixture-GGUF";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const FILE: &str = "fixture.gguf";
const SIZE: usize = 6 * CHUNK as usize + 4_321;
/// The mock's model host, and the storage host its resolve URL redirects to,
/// as `updated` takes them in the `model-download` case.
const MOCK_HOSTS: &[Hosts] = &[fetch::FIXTURE];

/// The model's bytes: a GGUF magic, then a deterministic stream.
fn model() -> Vec<u8> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut bytes: Vec<u8> = (0..SIZE)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    bytes[..4].copy_from_slice(b"GGUF");
    bytes
}

fn resolve_path() -> String {
    format!("/{HF_REPO}/resolve/{REVISION}/{FILE}")
}

/// Where the storage host keeps the bytes.
fn storage_path() -> String {
    format!("/xet/{REVISION}/{FILE}")
}

/// The mock as Hugging Face is: the resolve URL redirects, the storage host
/// serves.
fn content(bytes: Vec<u8>) -> Content {
    Content::new()
        .redirect(
            MODELS,
            &resolve_path(),
            &format!("https://{ASSETS}{}", storage_path()),
        )
        .file(ASSETS, &storage_path(), bytes)
}

/// The role's one target: the model at the mock's pinned revision.
fn role(bytes: &[u8]) -> Vec<Signed> {
    vec![Signed {
        id: "fixture-dl".into(),
        name: "Download fixture".into(),
        length: bytes.len() as u64,
        sha256: Sha256::digest(bytes).into(),
        licence: "MIT".into(),
        source: format!("https://{MODELS}{}", resolve_path()),
    }]
}

static EMPTY: Store = Store {
    blobs: Vec::new(),
    running: None,
    next: None,
    on_trial: false,
    rolled_back: None,
};

fn machine() -> Machine<'static> {
    Machine {
        memory_mib: 6 * 1024,
        free_bytes: 8 << 30,
        store: &EMPTY,
        partials: &[],
        hosts: MOCK_HOSTS,
    }
}

fn job(bytes: &[u8]) -> Job {
    fetch::plan(&role(bytes), &machine(), "fixture-dl").unwrap()
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

/// The model store's volume, in memory: the job's partial file and its
/// checkpoint, and the files moved in.
struct Volume<'a> {
    job: &'a Job,
    files: BTreeMap<String, Vec<u8>>,
    done: usize,
}

impl<'a> Volume<'a> {
    fn new(job: &'a Job) -> Self {
        Self {
            job,
            files: BTreeMap::new(),
            done: 0,
        }
    }
}

impl Partial for Volume<'_> {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        Ok(self.done)
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.done = done;
        Ok(())
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        let file = self.files.entry(self.job.partial.clone()).or_default();
        let end = offset as usize + bytes.len();
        if file.len() < end {
            file.resize(end, 0);
        }
        file[offset as usize..end].copy_from_slice(bytes);
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        let file = self
            .files
            .get(&self.job.partial)
            .ok_or(DownloadError::Store(SHORT_READ))?;
        let bytes = file
            .get(offset as usize..offset as usize + buf.len())
            .ok_or(DownloadError::Store(SHORT_READ))?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        let whole = self
            .files
            .remove(&self.job.partial)
            .ok_or(DownloadError::Store("no partial file"))?;
        self.files.insert(self.job.path.clone(), whole);
        Ok(())
    }

    fn size(&mut self) -> Result<u64, DownloadError> {
        let file = self.files.get(&self.job.partial);
        Ok(file.map_or(0, |bytes| bytes.len() as u64))
    }
}

/// One run of `job` from `mock`, reaching only the hosts the job does, on
/// every hop of the redirect.
fn run(
    mock: &Mock,
    job: &Job,
    list: &Chunks,
    volume: &mut Volume,
) -> Result<Report, DownloadError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(mock.ca().to_vec()))
        .map_err(|_| DownloadError::Store("the CA"))?;
    let wall = Wall {
        now_s: at_2030,
        floor_s: client::FLOOR_S,
    };
    let config = client::config(pki::provider(), roots, Arc::new(wall))
        .map_err(|_| DownloadError::Store("the config"))?;
    let mut connect = TlsConnect {
        config,
        dial: Local(mock.port),
    };
    let zero = || 0;
    let allow = |host: &str, port: u16| job.reaches(host, port);
    let fetch = Fetch {
        limits: Limits::new(CHUNK, u64::MAX),
        allow: &allow,
        clock: &zero,
    };
    let url = Url::parse(&job.source.url()).map_err(|_| DownloadError::Store("the URL"))?;
    download(list, volume, &mut |range: Range, sink| {
        // A status other than 206 is kainet's to refuse (`DownloadError::Status`).
        let got = fetch.run(&mut connect, Method::Get, &url, Some(range), sink)?;
        Ok(Answer::from(&got))
    })
}

/// Ranges the storage host served.
fn served(mock: &Mock) -> usize {
    mock.served()
        .into_iter()
        .filter(|s| s.host == ASSETS && s.path == storage_path())
        .count()
}

#[test]
fn models_chunk_is_kainets() {
    assert_eq!(MODEL_CHUNK, CHUNK);
}

#[test]
fn updateds_mock_hosts_are_the_mocks() {
    assert_eq!(
        fetch::FIXTURE,
        Hosts {
            source: MODELS,
            storage: &[ASSETS],
        }
    );
}

#[test]
fn two_cuts_resume_through_the_redirect_and_the_model_moves_into_the_store_whole() {
    let bytes = model();
    let job = job(&bytes);
    // The role's sibling target `<sha256>.chunks`, read back as Models reads
    // it before any range is fetched.
    let signed = Chunks::of(&bytes, CHUNK).render();
    let list = Chunks::parse(signed.as_bytes()).unwrap();
    job.check_list(&list.blob, list.size, list.chunk).unwrap();
    let mock = start(content(bytes.clone())).unwrap();
    let mut cuts = vec![SIZE * 8 / 10, SIZE * 4 / 10];
    mock.set_fault(ASSETS, &storage_path(), Fault::CutOnce(cuts.pop().unwrap()));
    let mut volume = Volume::new(&job);
    let mut resumes = 0;
    let report = loop {
        match run(&mock, &job, &list, &mut volume) {
            Ok(report) => break report,
            Err(DownloadError::Fetch(HttpError::Truncated)) => {
                resumes += 1;
                assert!(
                    !volume.files.contains_key(&job.path),
                    "nothing in the store yet"
                );
                if let Some(cut) = cuts.pop() {
                    mock.set_fault(ASSETS, &storage_path(), Fault::CutOnce(cut));
                }
            }
            Err(other) => panic!("{other:?}"),
        }
        assert!(resumes <= 2, "more resumes than cuts");
    };
    assert_eq!(resumes, 2);
    assert_eq!(report.kept + report.fetched, list.count());
    assert!(report.kept > 0, "the last run kept what the cuts left");
    // The partial file moved in, as the role signed it.
    assert!(!volume.files.contains_key(&job.partial));
    let stored = &volume.files[&job.path];
    assert_eq!(stored, &bytes);
    let sha: [u8; 32] = Sha256::digest(stored).into();
    assert_eq!(sha, job.sha256);
    assert_eq!(
        served(&mock),
        list.count() + 2,
        "one fetch more for each cut"
    );
}

#[test]
fn a_redirect_off_the_jobs_hosts_is_refused_and_nothing_is_stored() {
    let bytes = model();
    let job = job(&bytes);
    let list = Chunks::of(&bytes, CHUNK);
    let mock = start(content(bytes)).unwrap();
    mock.set_fault(MODELS, &resolve_path(), Fault::RedirectOff);
    let mut volume = Volume::new(&job);
    assert_eq!(
        run(&mock, &job, &list, &mut volume),
        Err(DownloadError::Fetch(HttpError::Refused(OFF.into())))
    );
    assert_eq!(volume.done, 0);
    assert!(volume.files.is_empty());
    assert_eq!(served(&mock), 0);
}

#[test]
fn a_list_for_other_bytes_or_other_chunks_is_refused_before_any_byte_is_fetched() {
    let bytes = model();
    let job = job(&bytes);
    let mut other = bytes.clone();
    other[100] ^= 1;
    let list = Chunks::of(&other, CHUNK);
    let mock = start(content(bytes.clone())).unwrap();
    assert_eq!(
        job.check_list(&list.blob, list.size, list.chunk),
        Err(Refused::List("the chunk list is for another blob"))
    );
    let wide = Chunks::of(&bytes, 4 * CHUNK);
    assert_eq!(
        job.check_list(&wide.blob, wide.size, wide.chunk),
        Err(Refused::List("the chunk list's chunks are not 2 MiB"))
    );
    assert_eq!(served(&mock), 0);
}

#[test]
fn a_flipped_byte_is_refused_by_its_chunk_and_nothing_reaches_the_store() {
    let bytes = model();
    let job = job(&bytes);
    let list = Chunks::of(&bytes, CHUNK);
    let mock = start(content(bytes)).unwrap();
    mock.set_fault(ASSETS, &storage_path(), Fault::Flip(2 * CHUNK as usize + 9));
    let mut volume = Volume::new(&job);
    assert_eq!(
        run(&mock, &job, &list, &mut volume),
        Err(DownloadError::Chunk(2))
    );
    assert_eq!(volume.done, 2);
    assert!(!volume.files.contains_key(&job.path));
}

#[test]
fn the_one_ask_names_the_mock_host_and_the_size() {
    let bytes = model();
    let role = role(&bytes);
    let offers = fetch::offers(&role, &machine());
    assert!(offers[0].recommended);
    let job = job(&bytes);
    assert_eq!(job.hosts, [MODELS, ASSETS]);
    assert_eq!(
        fetch::ask(&role[0], &job.source),
        format!(
            "Download Download fixture (12.0 MiB) from {MODELS}? Each part is checked against the signed models role before it is kept."
        )
    );
}
