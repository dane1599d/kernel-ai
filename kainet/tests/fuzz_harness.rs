//! The fuzz targets' bodies (`fuzz/harness.rs`) over hostile mutations of
//! good inputs, on the host: each returns for every input, so the targets
//! build and the harnesses hold before libFuzzer runs them on GitHub
//! (`.github/workflows/fuzz.yml`).

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/harness.rs"]
mod harness;

use kainet::chunks::Chunks;

/// Mutations of each sample; the fuzz workflow goes much further.
const CASES: usize = 2_000;

#[test]
fn the_tls_harnesses_return_for_mutated_records() {
    // A handshake record and an application-data record's shapes.
    let hello = [
        0x16, 0x03, 0x03, 0x00, 0x30, 0x02, 0x00, 0x00, 0x2c, 0x03, 0x03,
    ];
    let data = [0x17, 0x03, 0x03, 0x00, 0x20, 0x01, 0x02, 0x03, 0x04];
    hostile::fuzz(&hello, 0x7_15c1, CASES, harness::tls_client);
    hostile::fuzz(&data, 0x7_15c2, CASES / 10, harness::tls_records);
    harness::tls_client(&[]);
    harness::tls_records(&[]);
}

#[test]
fn the_http_harness_returns_for_mutated_responses() {
    for sample in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"[..],
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nwiki\r\n0\r\n\r\n",
        b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/9\r\nContent-Length: 4\r\n\r\nabcd",
        b"HTTP/1.1 302 Found\r\nLocation: /next\r\n\r\n",
    ] {
        hostile::fuzz(sample, 0x4_77b0, CASES, harness::http_response);
        hostile::truncations(sample, |_, bytes| harness::http_response(bytes));
    }
}

#[test]
fn the_list_and_url_harnesses_return_for_mutated_input() {
    let list = Chunks::of(&[7u8; 200_000], 64 * 1024).render();
    hostile::fuzz(list.as_bytes(), 0x6_c4c5, CASES, harness::kai_chunks);
    for sample in [
        "https://a.test/x?y",
        "https://releases.kai.test:8443/dl/v1/a.kpkg",
    ] {
        hostile::fuzz(sample.as_bytes(), 0x0_0e1, CASES, harness::url);
    }
}

#[test]
fn the_wire_harness_returns_for_mutated_messages() {
    for sample in [
        "get https://releases.kai.test/a.kpkg range=0-2097151 hops=2",
        "head https://updates.kai.test/metadata/timestamp.json",
        "ok 200\nlen 5\nhops 1\ntype text/plain\nretry 30\nurl https://a.test/x\n",
        "ok 302\nlen 0\nhops 0\nurl https://a.test/x\nlocation https://b.test/y\n",
        "err refused elsewhere.kai.test",
        "err truncated",
    ] {
        hostile::fuzz(sample.as_bytes(), 0x3_317e, CASES, harness::fetch_wire);
        hostile::truncations(sample.as_bytes(), |_, bytes| harness::fetch_wire(bytes));
    }
    harness::fetch_wire(&[]);
}
