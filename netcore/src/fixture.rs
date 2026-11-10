//! The test DNS zone: what xtask's responder answers and the kernel's
//! `net-setup` case expects, so neither needs the internet.

use alloc::string::String;
use alloc::vec::Vec;
use smoltcp::wire::{DnsPacket, DnsQueryType, DnsQuestion};

/// An A record with a long TTL: asked once, then served from the cache.
pub const KAI: &str = "kai.test";
pub const KAI_ADDRESS: [u8; 4] = [10, 0, 2, 99];
pub const KAI_TTL: u32 = 300;
/// An A record whose 1 s TTL runs out during the case.
pub const SHORT: &str = "short.kai.test";
pub const SHORT_ADDRESS: [u8; 4] = [10, 0, 2, 98];
pub const SHORT_TTL: u32 = 1;
/// A CNAME to [`KAI`], with the A record in the same reply.
pub const ALIAS: &str = "alias.kai.test";
pub const ALIAS_TTL: u32 = 60;
/// NXDOMAIN, as is every name not listed here.
pub const MISSING: &str = "missing.kai.test";
/// Never answered, so the resolver times out.
pub const SILENT: &str = "silent.kai.test";

/// xtask's HTTPS mock (P5 task 7): every host it serves is at one address,
/// which QEMU forwards, on port 443, to the mock on the host (P5 task 12's
/// harness, `xtask/src/cases/https.rs`). The names are the mock's own.
pub const MOCK_ADDRESS: [u8; 4] = [10, 0, 2, 100];
pub const MOCK_TTL: u32 = 300;
/// The update repository's Pages host.
pub const MOCK_REPO: &str = "updates.kai.test";
/// GitHub's releases host, which redirects downloads to [`MOCK_ASSETS`].
pub const MOCK_RELEASES: &str = "releases.kai.test";
/// GitHub's release-assets host.
pub const MOCK_ASSETS: &str = "assets.kai.test";
/// A Hugging Face-shaped model host.
pub const MOCK_MODELS: &str = "models.kai.test";
/// A host no handle names, where the mock's off-host redirect leads.
pub const MOCK_OFF: &str = "elsewhere.kai.test";
/// Every host of the mock.
pub const MOCK_HOSTS: &[&str] = &[MOCK_REPO, MOCK_RELEASES, MOCK_ASSETS, MOCK_MODELS, MOCK_OFF];

/// `net-download`'s release on the mock (P5 task 6's case): its job lists
/// are downloaded from [`MOCK_RELEASES`] at this path, as GitHub serves a
/// release's assets.
pub const DOWNLOAD_RELEASE: &str = "/dane1599d/kernel-ai-updates/releases/download/net-download/";
/// The job list a KVM boot runs, whose target is 32 chunks of 2 MiB, and its
/// SHA-256: pinned here as signed metadata pins a target. xtask generates the
/// targets from fixed seeds, and its tests check both pins.
pub const DOWNLOAD_JOBS: &str = "jobs";
pub const DOWNLOAD_JOBS_SHA256: &str =
    "a1d863bbb8e8ec97bd9f4cd0578b4425b72bd4ea13d16973fd15d8c143cc39cd";
/// The job list a TCG boot runs: the same 32 chunks at 64 KiB.
pub const DOWNLOAD_JOBS_SMALL: &str = "jobs-small";
pub const DOWNLOAD_JOBS_SMALL_SHA256: &str =
    "87f58ca23334b15c245d4ba279625e523f02e57580436b7c47b3b00842ca8b4d";

/// What an answer record holds.
#[derive(Clone, Copy)]
pub enum Data<'a> {
    A([u8; 4]),
    Cname(&'a str),
}

/// The responder's reply to `query`, or None to stay silent (for [`SILENT`],
/// and for anything that is not a single-question A query).
pub fn answer(query: &[u8]) -> Option<Vec<u8>> {
    let packet = DnsPacket::new_checked(query).ok()?;
    if packet.question_count() != 1 {
        return None;
    }
    let (_, question) = DnsQuestion::parse(packet.payload()).ok()?;
    if question.type_ != DnsQueryType::A {
        return None;
    }
    let host = name(&packet, question.name)?;
    let (rcode, answers): (u8, &[(&str, u32, Data)]) = match host.as_str() {
        SILENT => return None,
        KAI => (0, &[(KAI, KAI_TTL, Data::A(KAI_ADDRESS))]),
        SHORT => (0, &[(SHORT, SHORT_TTL, Data::A(SHORT_ADDRESS))]),
        ALIAS => (
            0,
            &[
                (ALIAS, ALIAS_TTL, Data::Cname(KAI)),
                (KAI, KAI_TTL, Data::A(KAI_ADDRESS)),
            ],
        ),
        MOCK_REPO => (0, &[(MOCK_REPO, MOCK_TTL, Data::A(MOCK_ADDRESS))]),
        MOCK_RELEASES => (0, &[(MOCK_RELEASES, MOCK_TTL, Data::A(MOCK_ADDRESS))]),
        MOCK_ASSETS => (0, &[(MOCK_ASSETS, MOCK_TTL, Data::A(MOCK_ADDRESS))]),
        MOCK_MODELS => (0, &[(MOCK_MODELS, MOCK_TTL, Data::A(MOCK_ADDRESS))]),
        MOCK_OFF => (0, &[(MOCK_OFF, MOCK_TTL, Data::A(MOCK_ADDRESS))]),
        _ => (3, &[]),
    };
    Some(reply(query, &host, rcode, answers))
}

/// A reply to `query` (whose question names `host`) with error code `rcode` and
/// `answers`. An owner equal to `host` is written as a pointer to the question,
/// as servers do; other names in full.
pub fn reply(query: &[u8], host: &str, rcode: u8, answers: &[(&str, u32, Data)]) -> Vec<u8> {
    let mut bytes = query.to_vec();
    bytes[2] |= 0x80;
    bytes[3] = 0x80 | (rcode & 0x0F);
    bytes[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());
    for (owner, ttl, data) in answers {
        if *owner == host {
            bytes.extend_from_slice(&[0xC0, 12]);
        } else {
            bytes.extend(wire_name(owner));
        }
        let (kind, body) = match data {
            Data::A(address) => (1u16, address.to_vec()),
            Data::Cname(target) => (5u16, wire_name(target)),
        };
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&ttl.to_be_bytes());
        bytes.extend_from_slice(&(body.len() as u16).to_be_bytes());
        bytes.extend(body);
    }
    bytes
}

fn wire_name(host: &str) -> Vec<u8> {
    let mut name = Vec::new();
    for label in host.split('.') {
        name.push(label.len() as u8);
        name.extend_from_slice(label.as_bytes());
    }
    name.push(0);
    name
}

fn name(packet: &DnsPacket<&[u8]>, wire: &[u8]) -> Option<String> {
    let mut text = String::new();
    for label in packet.parse_name(wire) {
        if !text.is_empty() {
            text.push('.');
        }
        text.extend(
            label
                .ok()?
                .iter()
                .map(|b| char::from(b.to_ascii_lowercase())),
        );
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::{ReplyError, parse_reply, query};
    use core::net::Ipv4Addr;

    #[test]
    fn the_zone_answers_as_the_kernel_case_expects() {
        let kai = parse_reply(1, KAI, &answer(&query(1, KAI)).unwrap()).unwrap();
        assert_eq!(kai.addresses, [Ipv4Addr::from(KAI_ADDRESS)]);
        assert_eq!(kai.ttl, KAI_TTL);
        let short = parse_reply(2, SHORT, &answer(&query(2, SHORT)).unwrap()).unwrap();
        assert_eq!(
            (short.addresses[0], short.ttl),
            (Ipv4Addr::from(SHORT_ADDRESS), SHORT_TTL)
        );
        let alias = parse_reply(3, ALIAS, &answer(&query(3, ALIAS)).unwrap()).unwrap();
        assert_eq!(
            (alias.addresses[0], alias.ttl),
            (Ipv4Addr::from(KAI_ADDRESS), ALIAS_TTL)
        );
        let missing = answer(&query(4, MISSING)).unwrap();
        assert_eq!(
            parse_reply(4, MISSING, &missing),
            Err(ReplyError::NoSuchHost)
        );
        assert!(answer(&query(5, SILENT)).is_none());
        assert!(answer(b"not dns").is_none());
    }

    #[test]
    fn every_mock_host_is_at_the_forwarded_address() {
        for (id, host) in (10u16..).zip(MOCK_HOSTS) {
            let got = parse_reply(id, host, &answer(&query(id, host)).unwrap()).unwrap();
            assert_eq!(
                (got.addresses[..].to_vec(), got.ttl),
                (alloc::vec![Ipv4Addr::from(MOCK_ADDRESS)], MOCK_TTL),
                "{host}"
            );
        }
    }
}
