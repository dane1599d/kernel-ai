//! `model-download` (P5 task 9): `updated` downloads a target of the signed
//! `models` role from the HTTPS mock as Hugging Face serves a model, into the
//! model store, inside the OS.
//!
//! The image's role signs `fixture-dl` (`pkgrepo::models::fixture_dl`), whose
//! pinned source is the mock's resolve URL on `models.kai.test`; that URL
//! redirects to the storage path on `assets.kai.test`, which serves the bytes,
//! and the role's chunk list is on the repository host's targets. `updated`
//! (`updated=download`, with `fetch=test` and a `NetDest` for those three
//! hosts) says what it offers, asks once, fetches the list and holds it to the
//! role, then fetches the model a chunk at a time. The storage host cuts the
//! download at 40% and 80%, and each cut is tried again from the chunk in
//! flight. The model is copied into the store's sealed blob, and `updated`
//! prints the bytes, the time and MB/s, the founder's throughput figure.

use super::{Https, TestCase, case};
use crate::blklog::Data;
use crate::mock_https::{ASSETS, Content, Fault, MODELS, REPO};
use crate::pkgrepo::models::{
    FIXTURE_DL_FILE, FIXTURE_DL_LEN, FIXTURE_DL_REPO, FIXTURE_DL_REVISION, FIXTURE_DL_STORAGE,
    fixture_dl_bytes,
};
use kainet::chunks::{CHUNK, Chunks};

const DATA: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// `kai-data` with room for the partial file, the store's sealed copy and
/// `modelchan::fetch::DISK_MARGIN`'s 256 MiB, with the volumes' own.
const DATA_MIB: u64 = 512;

/// The storage host cuts the model's download at 40% and 80%.
const FAULTS: &[(&str, &str, Fault)] = &[(ASSETS, FIXTURE_DL_STORAGE, Fault::CutsAt(&[40, 80]))];

/// The mock as Hugging Face and the repository serve `fixture-dl`: the
/// resolve URL redirects to the storage host, which serves the bytes, and the
/// role's chunk list is a consistent-snapshot target on the repository host.
fn content() -> Content {
    let bytes = fixture_dl_bytes();
    let list = Chunks::of(&bytes, CHUNK).render().into_bytes();
    let target = format!(
        "/targets/{}.{}.chunks",
        crate::sha256_hex(&list),
        crate::sha256_hex(&bytes)
    );
    let resolve = format!("/{FIXTURE_DL_REPO}/resolve/{FIXTURE_DL_REVISION}/{FIXTURE_DL_FILE}");
    Content::new()
        .redirect(
            MODELS,
            &resolve,
            &format!("https://{ASSETS}{FIXTURE_DL_STORAGE}"),
        )
        .file(ASSETS, FIXTURE_DL_STORAGE, bytes)
        .file(REPO, &target, list)
}

pub const CASES: &[TestCase] = &[TestCase {
    dns: true,
    data: Some(DATA),
    data_mib: Some(DATA_MIB),
    // The memory `fixture-dl` needs to run, as xtask sizes a VM for a model,
    // so `updated` offers it.
    memory_mib: Some(kabi::model::guest_mib(FIXTURE_DL_LEN)),
    https: Some(Https {
        content,
        faults: FAULTS,
    }),
    server: false,
    expect: &[
        "updated: models offers: fixture-dl 1 fits 0 0",
        "updated: models ask fixture-dl: ok Download 12.0 MiB from models.kai.test",
        "updated: downloading fixture-dl from models.kai.test",
        "updated: ok downloading fixture-dl 12587233 models.kai.test",
        "updated: model download, try 1 of 8 again: ",
        // The founder's throughput figure: bytes fetched over the whole time,
        // the list, the checks and the copy into the store included, with a
        // TLS connection for each 2 MiB chunk.
        "updated: downloaded fixture-dl (",
        " MB/s (7 chunks fetched, 0 kept, a TLS connection each)",
        "updated: models progress: ok downloaded fixture-dl",
        "updated: the store holds fixture-dl, as the role signs it",
        "updated: the boot record answered at once",
    ],
    ..case("model-download", "test=update updated=download")
}];
