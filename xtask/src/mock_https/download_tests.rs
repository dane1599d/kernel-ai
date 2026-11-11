//! The host half of P5's `net-download` case: `kainet`'s verified download
//! over its HTTP client and TLS, from the mock, as `pkg` and Models will run
//! it. A release asset is fetched in 2 MiB chunks by `Range`, each through
//! GitHub's redirect from the releases host to the assets host, which the
//! caller names; the mock cuts the connection at 40% and 80% of the file,
//! flips a byte, or the caller does not name the assets host.

use super::*;
use kainet::chunks::{CHUNK, Chunks};
use kainet::client::{self, Wall};
use kainet::download::{Answer, DownloadError, Partial, Report, download};
use kainet::http::{Dial, Fetch, HttpError, Limits, Method, Range, TlsConnect, Url};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

/// 2030-01-01, inside the mock's good certificates.
fn at_2030() -> Option<u64> {
    Some(1_893_456_000)
}

const TAG: &str = "v0.2.0";
const NAME: &str = "model.gguf";
const SIZE: usize = 10 * CHUNK as usize + 12_345;

fn target() -> Vec<u8> {
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    (0..SIZE)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn asset_path() -> String {
    format!("/assets/{TAG}/{NAME}")
}

fn download_url() -> Url {
    let path = format!("/dane1599d/kernel-ai-updates/releases/download/{TAG}/{NAME}");
    Url::parse(&format!("https://{RELEASES}{path}")).unwrap()
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
        let src = self
            .bytes
            .get(start..start + buf.len())
            .ok_or(DownloadError::Store("past the end"))?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        self.finished = true;
        Ok(())
    }
}

/// One run of the download from `mock`, reaching only the hosts `allow`
/// takes.
fn run(
    mock: &Mock,
    list: &Chunks,
    partial: &mut Memory,
    allow: &dyn Fn(&str, u16) -> bool,
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
    let fetch = Fetch {
        limits: Limits::new(CHUNK, u64::MAX),
        allow,
        clock: &zero,
    };
    let url = download_url();
    download(list, partial, &mut |range: Range, sink| {
        let got = fetch.run(&mut connect, Method::Get, &url, Some(range), sink)?;
        Ok(Answer::from(&got))
    })
}

fn github(host: &str, port: u16) -> bool {
    port == 443 && (host == RELEASES || host == ASSETS)
}

/// The ranges the assets host served, in order.
fn served_ranges(mock: &Mock) -> Vec<(usize, usize)> {
    mock.served()
        .into_iter()
        .filter(|s| s.host == ASSETS && s.path == asset_path())
        .filter_map(|s| s.range)
        .collect()
}

#[test]
fn cuts_at_forty_and_eighty_percent_resume_fetching_only_the_chunk_in_flight() {
    let target = target();
    let list = Chunks::of(&target, CHUNK);
    let mock = start(Content::new().release_asset(TAG, NAME, target.clone())).unwrap();
    let mut partial = Memory::default();
    let mut cuts = vec![SIZE * 8 / 10, SIZE * 4 / 10];
    mock.set_fault(ASSETS, &asset_path(), Fault::CutOnce(cuts.pop().unwrap()));
    let mut resumes = 0;
    let report = loop {
        match run(&mock, &list, &mut partial, &github) {
            Ok(report) => break report,
            Err(DownloadError::Fetch(HttpError::Truncated)) => {
                resumes += 1;
                if let Some(cut) = cuts.pop() {
                    mock.set_fault(ASSETS, &asset_path(), Fault::CutOnce(cut));
                }
            }
            Err(other) => panic!("{other:?}"),
        }
        assert!(resumes <= 2, "more resumes than cuts");
    };
    assert_eq!(resumes, 2);
    assert_eq!(
        report,
        Report {
            kept: 8,
            fetched: 3
        }
    );
    assert!(partial.finished);
    assert_eq!(partial.bytes, target);
    let ranges = served_ranges(&mock);
    assert_eq!(
        ranges.len(),
        list.count() + 2,
        "one fetch more for each cut"
    );
    let fourth = list.range(4);
    let again = ranges
        .iter()
        .filter(|r| (r.0 as u64, r.1 as u64) == fourth)
        .count();
    assert_eq!(again, 2, "the chunk the first cut was in came twice");
}

/// One `CutsAt` fault does what the two `CutOnce` faults above do, as the
/// boot case sets it before the boot.
#[test]
fn one_fault_cuts_at_forty_and_eighty_percent_in_turn() {
    let target = target();
    let list = Chunks::of(&target, CHUNK);
    let mock = start(Content::new().release_asset(TAG, NAME, target.clone())).unwrap();
    mock.set_fault(ASSETS, &asset_path(), Fault::CutsAt(&[40, 80]));
    let mut partial = Memory::default();
    let mut cuts = 0;
    let report = loop {
        match run(&mock, &list, &mut partial, &github) {
            Ok(report) => break report,
            Err(DownloadError::Fetch(HttpError::Truncated)) => cuts += 1,
            Err(other) => panic!("{other:?}"),
        }
        assert!(cuts <= 2, "more cuts than points");
    };
    assert_eq!((cuts, report.kept + report.fetched), (2, list.count()));
    assert_eq!(partial.bytes, target);
    assert_eq!(served_ranges(&mock).len(), list.count() + 2);
}

#[test]
fn a_flipped_byte_is_refused_by_its_chunk() {
    let target = target();
    let list = Chunks::of(&target, CHUNK);
    let mock = start(Content::new().release_asset(TAG, NAME, target)).unwrap();
    mock.set_fault(ASSETS, &asset_path(), Fault::Flip(3 * CHUNK as usize + 17));
    let mut partial = Memory::default();
    assert_eq!(
        run(&mock, &list, &mut partial, &github),
        Err(DownloadError::Chunk(3))
    );
    assert_eq!((partial.done, partial.finished), (3, false));
}

#[test]
fn a_caller_that_does_not_name_the_assets_host_never_reaches_it() {
    let target = target();
    let list = Chunks::of(&target, CHUNK);
    let mock = start(Content::new().release_asset(TAG, NAME, target)).unwrap();
    let only_releases = |host: &str, _: u16| host == RELEASES;
    let mut partial = Memory::default();
    assert_eq!(
        run(&mock, &list, &mut partial, &only_releases),
        Err(DownloadError::Fetch(HttpError::Refused(ASSETS.into())))
    );
    assert!(served_ranges(&mock).is_empty());
}

#[test]
fn a_busy_server_says_when_to_come_back() {
    let target = target();
    let list = Chunks::of(&target, CHUNK);
    let mock = start(Content::new().release_asset(TAG, NAME, target)).unwrap();
    mock.set_fault(ASSETS, &asset_path(), Fault::TooMany(5));
    let mut partial = Memory::default();
    assert_eq!(
        run(&mock, &list, &mut partial, &github),
        Err(DownloadError::Status(429, Some(5)))
    );
    assert_eq!(partial.done, 0);
}
