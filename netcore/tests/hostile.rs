//! Hostile DNS replies: `parse_reply` runs in ring 0 on bytes any server on
//! the link can send (P5 plan section 6). Every prefix, every field set to an
//! extreme value and seeded mutations of a CNAME-plus-A reply and of a reply
//! with more A records than are kept: an answer or an error, never a panic,
//! never more than `MAX_ADDRESSES` addresses, and an allocation bounded by
//! the input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use core::net::Ipv4Addr;
use netcore::dns::{MAX_ADDRESSES, parse_reply};

const ID: u16 = 0x4B41;
const HOST: &str = "www.kai.test";
/// Seeded mutations of each sample.
const MUTATIONS: usize = 20_000;
/// The most a parse may allocate: this many times the reply, plus a floor
/// for its fixed structures.
const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 4096;

/// `name` as DNS labels, ending in the root.
fn labels(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

fn header(answers: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ID.to_be_bytes());
    // A response, recursion desired and available, no error.
    out.extend_from_slice(&0x8180u16.to_be_bytes());
    for count in [1, answers, 0, 0] {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend_from_slice(&labels(HOST));
    out.extend_from_slice(&[0, 1, 0, 1]);
    out
}

/// A record with a compressed owner name at `owner`: its type, TTL and data.
fn record(out: &mut Vec<u8>, owner: usize, kind: u16, ttl: u32, data: &[u8]) {
    out.extend_from_slice(&(0xC000 | owner as u16).to_be_bytes());
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

/// `www.kai.test` is a CNAME for `cdn.kai.test`, which has one A record.
fn cname_and_a() -> Vec<u8> {
    let mut out = header(2);
    let cname_at = out.len() + 12;
    record(&mut out, 12, 5, 300, &labels("cdn.kai.test"));
    record(&mut out, cname_at, 1, 60, &[10, 0, 2, 99]);
    out
}

/// Twelve A records for the host: more than are kept.
fn many_a() -> Vec<u8> {
    let mut out = header(12);
    for n in 0..12u8 {
        record(&mut out, 12, 1, 30, &[10, 0, 2, n]);
    }
    out
}

/// One parse of `bytes`: whatever it answers stays in bounds.
fn check(bytes: &[u8]) {
    let (result, peak) = hostile::peak_alloc(|| parse_reply(ID, HOST, bytes));
    if let Ok(answer) = &result {
        assert!(
            answer.addresses.len() <= MAX_ADDRESSES,
            "{} addresses from {bytes:02x?}",
            answer.addresses.len()
        );
    }
    let bound = bytes.len() * ALLOC_PER_BYTE + ALLOC_FLOOR;
    assert!(
        peak <= bound,
        "{peak} bytes allocated for a {}-byte reply",
        bytes.len()
    );
}

#[test]
fn the_samples_parse_as_built() {
    let answer = parse_reply(ID, HOST, &cname_and_a()).expect("the CNAME reply");
    assert_eq!(answer.addresses, [Ipv4Addr::new(10, 0, 2, 99)]);
    assert_eq!(answer.ttl, 60);
    let answer = parse_reply(ID, HOST, &many_a()).expect("the long reply");
    assert_eq!(answer.addresses.len(), MAX_ADDRESSES);
}

#[test]
fn every_prefix_is_an_answer_or_an_error() {
    for sample in [cname_and_a(), many_a()] {
        hostile::truncations(&sample, |cut, prefix| {
            check(prefix);
            if cut < sample.len() {
                // Nothing cut short reads as the whole reply's answer.
                let whole = parse_reply(ID, HOST, &sample);
                if let Ok(answer) = parse_reply(ID, HOST, prefix) {
                    assert_ne!(Ok(answer), whole, "cut at {cut}");
                }
            }
        });
    }
}

#[test]
fn every_field_at_every_extreme_stays_in_bounds() {
    for sample in [cname_and_a(), many_a()] {
        hostile::sweep_fields(&sample, 0..sample.len(), |_, bytes| check(bytes));
    }
}

#[test]
fn seeded_mutations_stay_in_bounds() {
    hostile::fuzz(&cname_and_a(), 0xD45, MUTATIONS, check);
    hostile::fuzz(&many_a(), 0xD46, MUTATIONS, check);
}
