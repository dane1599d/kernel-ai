//! The host half of P5's `update-online` case (task 10): `updatenet` against the
//! mock, with a test-signed repository served as GitHub serves
//! `kernel-ai-updates` (the TUF metadata on the Pages host, the release's image
//! and its chunk list as assets of one Release behind the releases host's
//! redirect). The device is `updated` with a memory slot: `kaituf` refreshes the
//! metadata through `kaipkg::online`, the release must be newer than the one the
//! boot record holds, the chunk list is held to its signed target, and the image
//! streams into the idle slot by `Range`, each chunk checked before it is
//! written. The mock cuts the connection, flips a byte, answers 429, redirects
//! off the repository's hosts, serves a certificate for another name, or serves
//! metadata that was changed after it was signed.

use super::*;
use crate::image::update_disk::test_root;
use bootslot::{Nv, Reason, Record, Slot, SlotState};
use kainet::chunks::{CHUNK, Chunks, MAX_LIST};
use kainet::client::{self, Wall};
use kainet::http::{Dial, HttpError, TlsConnect, Url};
use kaipkg::memvol::MemVolume;
use kaipkg::mirror::{TUF_DIR, TufCache};
use kaipkg::online::{Hosts, Online};
use kaipkg::store::Volume;
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use updatenet::net::Over;
use updatenet::plan::{self, Machine, Refused};
use updatenet::slot::{Checkpoint, Mark, SlotIo, SlotPartial};
use updatenet::stage::{Bad, Boot, Staged, Stop, stage};

/// 2030-01-01 for TLS, inside the mock's good certificates; the TUF metadata
/// keeps its own time ([`TestRoot::clock`]).
fn at_2030() -> Option<u64> {
    Some(1_893_456_000)
}

const TAG: &str = "os-7";
const SEQ: u32 = 7;
/// Five whole chunks and a short last one.
const SIZE: usize = 5 * CHUNK as usize + 4_321;
const REQUEST_NS: u64 = 30_000_000_000;

fn image() -> Vec<u8> {
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

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

/// A release as the repository publishes it: its bytes, its chunk list, and the
/// signed metadata for them.
struct Published {
    image: Vec<u8>,
    list: String,
    files: Vec<(String, Vec<u8>)>,
}

impl Published {
    /// Release `seq`, with the chunk list of `listed` (the image itself unless a
    /// test signs a list for other bytes).
    fn new(image: Vec<u8>, listed: &[u8], seq: u32) -> Self {
        let list = Chunks::of(listed, CHUNK).render();
        let files = test_root()
            .unwrap()
            .repository(&image, list.as_bytes(), seq, TAG)
            .unwrap();
        Self { image, list, files }
    }

    fn standard() -> Self {
        let image = image();
        Self::new(image.clone(), &image, SEQ)
    }

    /// Where the image is on the assets host.
    fn image_path(&self) -> String {
        format!("/assets/{TAG}/{}.slot.img", hex(&self.image))
    }

    /// Where the releases host has it, before its redirect.
    fn download_path(&self) -> String {
        format!(
            "/dane1599d/kernel-ai-updates/releases/download/{TAG}/{}.slot.img",
            hex(&self.image)
        )
    }

    fn root(&self) -> &[u8] {
        &self
            .files
            .iter()
            .find(|(p, _)| p == "metadata/1.root.json")
            .unwrap()
            .1
    }

    /// As GitHub would serve it.
    fn serve(&self) -> Mock {
        let mut content = Content::new();
        for (path, bytes) in &self.files {
            let name = path.strip_prefix("metadata/").expect("metadata");
            content = content.file(REPO, &format!("/metadata/{name}"), bytes.clone());
        }
        content = content
            .release_asset(
                TAG,
                &format!("{}.slot.img", hex(&self.image)),
                self.image.clone(),
            )
            .release_asset(
                TAG,
                &format!("{}.slot.img.chunks", hex(self.list.as_bytes())),
                self.list.clone().into_bytes(),
            );
        start(content).unwrap()
    }
}

fn online(mock: &Mock) -> Online<'static, TlsConnect<Local>> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(mock.ca().to_vec())).unwrap();
    let wall = Wall {
        now_s: at_2030,
        floor_s: client::FLOOR_S,
    };
    let config = client::config(pki::provider(), roots, Arc::new(wall)).unwrap();
    let connect = TlsConnect {
        config,
        dial: Local(mock.port),
    };
    Online::new(hosts(), connect, &|| 0, REQUEST_NS)
}

/// The idle slot, in memory.
struct Mem {
    bytes: Vec<u8>,
    writes: Vec<(u64, usize)>,
}

impl SlotIo for Mem {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
        self.writes.push((offset, bytes.len()));
        let end = offset as usize + bytes.len();
        self.bytes
            .get_mut(offset as usize..end)
            .ok_or("past the end of the slot")?
            .copy_from_slice(bytes);
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let end = offset as usize + buf.len();
        buf.copy_from_slice(self.bytes.get(offset as usize..end).ok_or("past the end")?);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        Ok(())
    }
}

#[derive(Default)]
struct Memo(Option<Mark>);

impl Checkpoint for Memo {
    fn load(&mut self) -> Result<Option<Mark>, &'static str> {
        Ok(self.0)
    }

    fn save(&mut self, mark: Option<Mark>) -> Result<(), &'static str> {
        self.0 = mark;
        Ok(())
    }
}

#[derive(Default)]
struct Var(Vec<Nv>);

impl Boot for Var {
    fn set(&mut self, nv: &Nv) -> Result<(), &'static str> {
        self.0.push(*nv);
        Ok(())
    }
}

/// A device: the slot, what survives a reboot, and the trust it holds.
struct Device {
    slot: Mem,
    memo: Memo,
    var: Var,
    nv: Nv,
    trust: MemVolume,
}

impl Device {
    /// Running slot A on release `committed`, trusting `published`'s root.
    fn new(published: &Published, committed: u32) -> Self {
        let mut record = Record::default();
        record.slots[Slot::A as usize] = SlotState {
            priority: 2,
            successful: true,
            release_seq: committed,
            ..SlotState::default()
        };
        let mut trust = MemVolume::default();
        trust
            .write(&format!("{TUF_DIR}/root.json"), published.root())
            .unwrap();
        Self {
            slot: Mem {
                bytes: vec![0; SIZE + 4096],
                writes: Vec::new(),
            },
            memo: Memo::default(),
            var: Var::default(),
            nv: Nv {
                record,
                one_shot: Some(Slot::A),
            },
            trust,
        }
    }

    fn written(&self) -> &[u8] {
        &self.slot.bytes[..SIZE]
    }
}

/// Why a run stopped.
#[derive(Debug, PartialEq)]
enum Failed {
    Refused(Refused),
    Fetch(String),
    Stop(Stop),
}

/// One run of `updated`'s update against `mock`: refresh, plan, fetch the chunk
/// list, stage.
fn run(mock: &Mock, device: &mut Device) -> Result<Staged, Failed> {
    let mut online = online(mock);
    let now = test_root().unwrap().clock();
    let mut cache = TufCache {
        vol: &mut device.trust,
    };
    let (image, list) = plan::refresh(&mut online, &mut cache, now).map_err(Failed::Refused)?;
    let machine = Machine {
        record: &device.nv.record,
        running: Slot::A,
        into: None,
        room: device.slot.bytes.len() as u64,
    };
    let job = plan::plan(&image, &list, &machine, &hosts()).map_err(Failed::Refused)?;
    let mut over = Over::of(&mut online);
    let (status, bytes) = over
        .small(&job.list_url, MAX_LIST as u64)
        .map_err(|e| Failed::Fetch(format!("{e:?}")))?;
    if status != 200 {
        return Err(Failed::Fetch(format!("the chunk list answered {status}")));
    }
    let chunks = job.chunks(&bytes).map_err(Failed::Refused)?;
    let url = Url::parse(&job.image_url).map_err(|e| Failed::Fetch(e.into()))?;
    let mut partial = SlotPartial::new(&mut device.slot, &mut device.memo, &job.release);
    // The metadata again, from its own connection, once the image has checked out.
    let mut recheck = || {
        let mut again = self::online(mock);
        let mut cache = TufCache {
            vol: &mut device.trust,
        };
        plan::refresh(&mut again, &mut cache, now)
    };
    stage(
        &job,
        &chunks,
        &mut device.nv,
        &mut device.var,
        &mut partial,
        &mut recheck,
        &mut |range, sink| over.range(&url, range, sink),
    )
    .map_err(Failed::Stop)
}

/// Requests the mock answered for the image's bytes.
fn image_ranges(mock: &Mock, published: &Published) -> usize {
    mock.served()
        .into_iter()
        .filter(|s| s.host == ASSETS && s.path == published.image_path() && s.range.is_some())
        .count()
}

fn assets_served(mock: &Mock) -> usize {
    mock.served()
        .into_iter()
        .filter(|s| s.host == ASSETS)
        .count()
}

#[test]
fn the_release_streams_in_through_the_redirect_and_becomes_a_trial_slot() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    let staged = run(&mock, &mut device).unwrap();
    assert_eq!((staged.slot, staged.release_seq), (Slot::B, SEQ));
    assert_eq!(staged.report.fetched, 6);
    assert_eq!(device.written(), &published.image[..]);
    // The slot was marked Updating before it was written, and is a trial last.
    assert_eq!(device.var.0.len(), 2);
    assert_eq!(
        device.var.0[0].record.slot(Slot::B).reason,
        Reason::Updating
    );
    let trial = device.var.0[1].record.slot(Slot::B);
    assert_eq!((trial.release_seq, trial.tries), (SEQ, bootslot::TRIES));
    assert!(bootslot::may_boot(&device.var.0[1].record, Slot::B));
    // Every request was to the repository's three hosts, and the image came a
    // chunk at a time.
    let served = mock.served();
    assert!(
        served
            .iter()
            .all(|s| [REPO, RELEASES, ASSETS].contains(&s.host.as_str())),
        "{served:?}"
    );
    assert_eq!(image_ranges(&mock, &published), 6);
    assert!(
        served
            .iter()
            .any(|s| s.host == REPO && s.path == "/metadata/timestamp.json")
    );
}

#[test]
fn two_cuts_resume_where_they_stopped_and_the_slot_is_whole_at_the_end() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    let mut cuts = vec![SIZE * 8 / 10, SIZE * 4 / 10];
    mock.set_fault(
        ASSETS,
        &published.image_path(),
        Fault::CutOnce(cuts.pop().unwrap()),
    );
    let mut resumes = 0;
    let staged = loop {
        match run(&mock, &mut device) {
            Ok(staged) => break staged,
            Err(Failed::Stop(Stop::Retry { .. })) => {
                resumes += 1;
                // Cut: the slot is Updating, never a trial, and the mark holds.
                assert!(!bootslot::may_boot(&device.nv.record, Slot::B));
                assert!(device.memo.0.is_some_and(|m| m.done > 0));
                if let Some(cut) = cuts.pop() {
                    mock.set_fault(ASSETS, &published.image_path(), Fault::CutOnce(cut));
                }
            }
            Err(other) => panic!("{other:?}"),
        }
        assert!(resumes <= 2, "more resumes than cuts");
    };
    assert_eq!(resumes, 2);
    assert!(
        staged.report.kept > 0,
        "the last run kept what the cuts left"
    );
    assert_eq!(device.written(), &published.image[..]);
    assert_eq!(
        image_ranges(&mock, &published),
        6 + 2,
        "one fetch more for each cut"
    );
    assert!(bootslot::may_boot(&device.nv.record, Slot::B));
}

#[test]
fn a_resume_after_a_reboot_needs_only_the_slot_the_mark_and_the_trust() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(
        ASSETS,
        &published.image_path(),
        Fault::CutOnce(3 * CHUNK as usize + 10),
    );
    assert!(matches!(
        run(&mock, &mut device),
        Err(Failed::Stop(Stop::Retry { .. }))
    ));
    // A reboot: the variable is read back from firmware; the rest is as it was.
    let mut again = Device {
        nv: device.var.0[0],
        var: Var::default(),
        ..device
    };
    let staged = run(&mock, &mut again).unwrap();
    assert_eq!((staged.report.kept, staged.report.fetched), (3, 3));
    assert_eq!(again.written(), &published.image[..]);
}

#[test]
fn a_flipped_byte_is_refused_by_its_chunk_and_nothing_after_it_is_written() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(
        ASSETS,
        &published.image_path(),
        Fault::Flip(2 * CHUNK as usize + 9),
    );
    assert_eq!(
        run(&mock, &mut device),
        Err(Failed::Stop(Stop::Bad(Bad::Chunk(2))))
    );
    assert!(
        device
            .slot
            .writes
            .iter()
            .all(|(offset, _)| *offset < 2 * CHUNK),
        "{:?}",
        device.slot.writes
    );
    assert!(
        device.slot.bytes[2 * CHUNK as usize..]
            .iter()
            .all(|b| *b == 0)
    );
    // Never a trial: the variable says Updating and no more.
    assert_eq!(device.var.0.len(), 1);
    assert!(!bootslot::may_boot(&device.nv.record, Slot::B));
    // The server fixed, the next run resumes at chunk 2.
    mock.clear_faults();
    let staged = run(&mock, &mut device).unwrap();
    assert_eq!(staged.report.kept, 2);
    assert_eq!(device.written(), &published.image[..]);
}

#[test]
fn a_busy_server_is_a_retry_with_its_wait() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(ASSETS, &published.image_path(), Fault::TooMany(7));
    let failed = run(&mock, &mut device).unwrap_err();
    assert!(
        matches!(
            &failed,
            Failed::Stop(Stop::Retry {
                after_s: Some(7),
                ..
            })
        ),
        "{failed:?}"
    );
    assert!(device.slot.writes.is_empty());
    mock.clear_faults();
    assert!(run(&mock, &mut device).is_ok());
}

#[test]
fn metadata_changed_after_it_was_signed_is_refused_and_no_asset_is_fetched() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(REPO, "/metadata/1.targets.json", Fault::Flip(200));
    let failed = run(&mock, &mut device).unwrap_err();
    assert!(
        matches!(failed, Failed::Refused(Refused::Repository(_))),
        "{failed:?}"
    );
    assert_eq!(assets_served(&mock), 0);
    assert!(device.slot.writes.is_empty());
    assert!(device.var.0.is_empty());
}

#[test]
fn metadata_signed_by_another_root_is_refused() {
    let published = Published::standard();
    let mock = published.serve();
    // A device that trusts another root.
    let mut device = Device::new(&published, SEQ - 1);
    let other = test_root().unwrap().root_json().unwrap();
    let mut changed = other.clone();
    let at = changed.len() / 2;
    changed[at] ^= 1;
    device
        .trust
        .write(&format!("{TUF_DIR}/root.json"), &changed)
        .unwrap();
    let failed = run(&mock, &mut device).unwrap_err();
    assert!(
        matches!(failed, Failed::Refused(Refused::Repository(_))),
        "{failed:?}"
    );
    assert_eq!(assets_served(&mock), 0);
}

#[test]
fn a_release_not_newer_than_the_committed_one_is_refused_before_any_asset() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ);
    assert_eq!(
        run(&mock, &mut device),
        Err(Failed::Refused(Refused::Rollback {
            seq: SEQ,
            newest: SEQ
        }))
    );
    // A committed floor above it does the same, whatever slot runs.
    let mut device = Device::new(&published, 1);
    device.nv.record.min_release_seq = SEQ + 3;
    assert!(matches!(
        run(&mock, &mut device),
        Err(Failed::Refused(Refused::Rollback { .. }))
    ));
    assert_eq!(assets_served(&mock), 0);
    assert!(device.var.0.is_empty());
}

#[test]
fn a_signed_list_for_other_bytes_is_refused_before_the_image_is_fetched() {
    let image = image();
    let mut other = image.clone();
    other[100] ^= 1;
    let published = Published::new(image, &other, SEQ);
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    assert_eq!(
        run(&mock, &mut device),
        Err(Failed::Refused(Refused::List(
            "the chunk list is for another image"
        )))
    );
    assert_eq!(image_ranges(&mock, &published), 0);
    assert!(device.slot.writes.is_empty());
    assert!(device.var.0.is_empty());
}

#[test]
fn a_redirect_off_the_repositorys_hosts_is_refused_and_the_slot_stays_unbootable() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(RELEASES, &published.download_path(), Fault::RedirectOff);
    let failed = run(&mock, &mut device).unwrap_err();
    assert!(
        matches!(&failed, Failed::Stop(Stop::Bad(Bad::Fetch(_)))),
        "{failed:?}"
    );
    assert!(device.slot.writes.is_empty());
    assert_eq!(device.var.0.len(), 1);
    assert!(!bootslot::may_boot(&device.nv.record, Slot::B));
    assert!(
        mock.served().iter().all(|s| s.host != OFF),
        "nothing was fetched from the other host"
    );
}

#[test]
fn a_certificate_for_another_name_is_refused() {
    let published = Published::standard();
    let mock = published.serve();
    let mut device = Device::new(&published, SEQ - 1);
    mock.set_fault(ASSETS, "", Fault::WrongName);
    let failed = run(&mock, &mut device).unwrap_err();
    assert!(
        matches!(
            &failed,
            Failed::Stop(Stop::Bad(Bad::Fetch(_))) | Failed::Fetch(_)
        ),
        "{failed:?}"
    );
    assert!(device.slot.writes.is_empty());
    assert!(!bootslot::may_boot(&device.nv.record, Slot::B));
}
