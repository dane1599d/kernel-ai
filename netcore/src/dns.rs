//! DNS A-record queries and replies, over smoltcp's DNS wire format.
//!
//! The kernel sends the query over a UDP socket of its own rather than smoltcp's
//! DNS socket: that socket only talks to port 53, which a test responder on the
//! host cannot bind, and keeps no TTL, which the cache needs.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use smoltcp::wire::{
    DnsFlags, DnsOpcode, DnsPacket, DnsQueryType, DnsQuestion, DnsRcode, DnsRecord, DnsRecordData,
    DnsRepr,
};

/// The longest host name, in its dotted form.
pub const MAX_NAME: usize = 253;
const MAX_LABEL: usize = 63;
/// The most addresses one answer keeps.
pub const MAX_ADDRESSES: usize = 8;
/// CNAME links followed before giving up on a loop.
const MAX_ALIASES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    /// A label that is empty, longer than 63 bytes, or not letters, digits and
    /// inner hyphens (underscores allowed, as some service names use them).
    BadLabel,
}

impl NameError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Empty => "dns: empty host name",
            Self::TooLong => "dns: host name longer than 253 characters",
            Self::BadLabel => "dns: host name has a bad label",
        }
    }
}

/// `host` in lower case without a trailing dot, if it is a valid host name.
pub fn normalize(host: &str) -> Result<String, NameError> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return Err(NameError::Empty);
    }
    if host.len() > MAX_NAME {
        return Err(NameError::TooLong);
    }
    for label in host.split('.') {
        let bytes = label.as_bytes();
        let inner = |b: &u8| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_';
        let ends = |b: Option<&u8>| b.is_some_and(|b| *b != b'-');
        if label.is_empty()
            || label.len() > MAX_LABEL
            || !bytes.iter().all(inner)
            || !ends(bytes.first())
            || !ends(bytes.last())
        {
            return Err(NameError::BadLabel);
        }
    }
    Ok(host.to_ascii_lowercase())
}

/// A normalized name in wire form: each label after its length, then a zero.
fn wire_name(host: &str) -> Vec<u8> {
    let mut name = Vec::with_capacity(host.len() + 2);
    for label in host.split('.') {
        name.push(label.len() as u8);
        name.extend_from_slice(label.as_bytes());
    }
    name.push(0);
    name
}

/// A recursive query for the A records of `host`, which must be normalized.
pub fn query(id: u16, host: &str) -> Vec<u8> {
    let name = wire_name(host);
    let repr = DnsRepr {
        transaction_id: id,
        opcode: DnsOpcode::Query,
        flags: DnsFlags::RECURSION_DESIRED,
        question: DnsQuestion {
            name: &name,
            type_: DnsQueryType::A,
        },
    };
    let mut bytes = vec![0u8; repr.buffer_len()];
    repr.emit(&mut DnsPacket::new_unchecked(&mut bytes[..]));
    bytes
}

/// What a reply said about the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// At most [`MAX_ADDRESSES`], in the server's order.
    pub addresses: Vec<Ipv4Addr>,
    /// The smallest TTL along the way, in seconds.
    pub ttl: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyError {
    /// Not a DNS response, or cut short.
    Malformed,
    /// A response to some other query: another id or question. The resolver
    /// keeps waiting for its own.
    NotOurs,
    /// NXDOMAIN.
    NoSuchHost,
    /// The name exists but has no A record.
    NoAddress,
    /// Too big for UDP; TCP is not done.
    Truncated,
    /// Any other error code the server sent.
    Server(u8),
}

impl ReplyError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Malformed => "dns: malformed reply",
            Self::NotOurs => "dns: reply to another query",
            Self::NoSuchHost => "dns: no such host",
            Self::NoAddress => "dns: the host has no IPv4 address",
            Self::Truncated => "dns: reply truncated",
            Self::Server(_) => "dns: the server failed",
        }
    }
}

/// The A records a reply to query `id` for `host` (normalized) gives, following
/// CNAMEs inside the reply.
pub fn parse_reply(id: u16, host: &str, bytes: &[u8]) -> Result<Answer, ReplyError> {
    let packet = DnsPacket::new_checked(bytes).map_err(|_| ReplyError::Malformed)?;
    if !packet.flags().contains(DnsFlags::RESPONSE) || packet.opcode() != DnsOpcode::Query {
        return Err(ReplyError::Malformed);
    }
    if packet.transaction_id() != id || packet.question_count() != 1 {
        return Err(ReplyError::NotOurs);
    }
    let (mut rest, question) =
        DnsQuestion::parse(packet.payload()).map_err(|_| ReplyError::Malformed)?;
    if question.type_ != DnsQueryType::A || flatten(&packet, question.name)? != host {
        return Err(ReplyError::NotOurs);
    }
    match packet.rcode() {
        DnsRcode::NoError => {}
        DnsRcode::NXDomain => return Err(ReplyError::NoSuchHost),
        other => return Err(ReplyError::Server(u8::from(other))),
    }
    if packet.flags().contains(DnsFlags::TRUNCATED) {
        return Err(ReplyError::Truncated);
    }
    let mut records = Vec::new();
    for _ in 0..packet.answer_record_count() {
        let (next, record) = DnsRecord::parse(rest).map_err(|_| ReplyError::Malformed)?;
        rest = next;
        let name = flatten(&packet, record.name)?;
        let data = match record.data {
            DnsRecordData::A(address) => Data::Address(address),
            DnsRecordData::Cname(target) => Data::Alias(flatten(&packet, target)?),
            _ => continue,
        };
        records.push((name, record.ttl, data));
    }
    chase(host, &records)
}

enum Data {
    Address(Ipv4Addr),
    Alias(String),
}

/// Follows CNAMEs from `host`, then takes the A records of the name reached.
fn chase(host: &str, records: &[(String, u32, Data)]) -> Result<Answer, ReplyError> {
    let mut name = host;
    let mut ttl = u32::MAX;
    for _ in 0..MAX_ALIASES {
        let alias = records.iter().find_map(|(owner, ttl, data)| match data {
            Data::Alias(target) if owner == name => Some((target.as_str(), *ttl)),
            _ => None,
        });
        let Some((target, alias_ttl)) = alias else {
            break;
        };
        name = target;
        ttl = ttl.min(alias_ttl);
    }
    let mut addresses = Vec::new();
    for (owner, record_ttl, data) in records {
        if let Data::Address(address) = data
            && owner == name
            && addresses.len() < MAX_ADDRESSES
        {
            addresses.push(*address);
            ttl = ttl.min(*record_ttl);
        }
    }
    if addresses.is_empty() {
        return Err(ReplyError::NoAddress);
    }
    Ok(Answer { addresses, ttl })
}

/// A name in a packet, following compression pointers, as lower-case dotted text.
fn flatten<T: AsRef<[u8]>>(packet: &DnsPacket<T>, name: &[u8]) -> Result<String, ReplyError> {
    let mut text = String::new();
    for label in packet.parse_name(name) {
        let label = label.map_err(|_| ReplyError::Malformed)?;
        if !text.is_empty() {
            text.push('.');
        }
        text.extend(label.iter().map(|b| char::from(b.to_ascii_lowercase())));
        if text.len() > MAX_NAME {
            return Err(ReplyError::Malformed);
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixture::{self, Data};

    /// A reply to `query` for `host`, its answers written as servers write them.
    fn reply(query: &[u8], host: &str, rcode: u8, answers: &[(&str, u32, Data)]) -> Vec<u8> {
        fixture::reply(query, host, rcode, answers)
    }

    #[test]
    fn names_are_checked_and_lowered() {
        assert_eq!(normalize("Kai.Test."), Ok("kai.test".into()));
        assert_eq!(normalize("a-b.c_d.example"), Ok("a-b.c_d.example".into()));
        assert_eq!(normalize(""), Err(NameError::Empty));
        assert_eq!(normalize("."), Err(NameError::Empty));
        for bad in ["a..b", "-a.b", "a-.b", "a b", "a.b/c", "ä.de"] {
            assert_eq!(normalize(bad), Err(NameError::BadLabel), "{bad}");
        }
        let long_label = "a".repeat(64);
        assert_eq!(normalize(&long_label), Err(NameError::BadLabel));
        let long_name = alloc::vec!["a".repeat(63); 4].join(".");
        assert_eq!(normalize(&long_name), Err(NameError::TooLong));
    }

    #[test]
    fn a_query_asks_recursively_for_one_a_record() {
        let bytes = query(0x1234, "kai.test");
        assert_eq!(&bytes[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&bytes[4..12], &[0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[12..], b"\x03kai\x04test\x00\x00\x01\x00\x01");
    }

    #[test]
    fn a_reply_gives_its_addresses_and_smallest_ttl() {
        let q = query(7, "kai.test");
        let r = reply(
            &q,
            "kai.test",
            0,
            &[
                ("kai.test", 300, Data::A([10, 0, 2, 99])),
                ("kai.test", 60, Data::A([10, 0, 2, 98])),
            ],
        );
        let answer = parse_reply(7, "kai.test", &r).unwrap();
        assert_eq!(
            answer.addresses,
            [Ipv4Addr::new(10, 0, 2, 99), Ipv4Addr::new(10, 0, 2, 98)]
        );
        assert_eq!(answer.ttl, 60);
    }

    #[test]
    fn cnames_are_followed_and_count_toward_the_ttl() {
        let q = query(9, "alias.kai.test");
        let r = reply(
            &q,
            "alias.kai.test",
            0,
            &[
                ("alias.kai.test", 30, Data::Cname("kai.test")),
                ("kai.test", 300, Data::A([10, 0, 2, 99])),
                ("other.test", 300, Data::A([1, 2, 3, 4])),
            ],
        );
        let answer = parse_reply(9, "alias.kai.test", &r).unwrap();
        assert_eq!(answer.addresses, [Ipv4Addr::new(10, 0, 2, 99)]);
        assert_eq!(answer.ttl, 30);
    }

    #[test]
    fn a_cname_loop_ends_without_addresses() {
        let q = query(9, "a.test");
        let r = reply(
            &q,
            "a.test",
            0,
            &[
                ("a.test", 30, Data::Cname("b.test")),
                ("b.test", 30, Data::Cname("a.test")),
            ],
        );
        assert_eq!(parse_reply(9, "a.test", &r), Err(ReplyError::NoAddress));
    }

    #[test]
    fn errors_and_other_queries_are_told_apart() {
        let q = query(3, "missing.kai.test");
        assert_eq!(
            parse_reply(
                3,
                "missing.kai.test",
                &reply(&q, "missing.kai.test", 3, &[])
            ),
            Err(ReplyError::NoSuchHost)
        );
        assert_eq!(
            parse_reply(
                3,
                "missing.kai.test",
                &reply(&q, "missing.kai.test", 2, &[])
            ),
            Err(ReplyError::Server(2))
        );
        assert_eq!(
            parse_reply(
                3,
                "missing.kai.test",
                &reply(&q, "missing.kai.test", 0, &[])
            ),
            Err(ReplyError::NoAddress)
        );
        assert_eq!(
            parse_reply(
                4,
                "missing.kai.test",
                &reply(&q, "missing.kai.test", 0, &[])
            ),
            Err(ReplyError::NotOurs)
        );
        assert_eq!(
            parse_reply(3, "kai.test", &reply(&q, "missing.kai.test", 0, &[])),
            Err(ReplyError::NotOurs)
        );
        // The query itself is not a response.
        assert_eq!(
            parse_reply(3, "missing.kai.test", &q),
            Err(ReplyError::Malformed)
        );
        let mut truncated = reply(&q, "missing.kai.test", 0, &[]);
        truncated[2] |= 0x02;
        assert_eq!(
            parse_reply(3, "missing.kai.test", &truncated),
            Err(ReplyError::Truncated)
        );
    }

    #[test]
    fn cut_or_garbled_replies_are_malformed() {
        let q = query(5, "kai.test");
        let r = reply(
            &q,
            "kai.test",
            0,
            &[("kai.test", 300, Data::A([10, 0, 2, 99]))],
        );
        for len in [0, 11, 20, r.len() - 1] {
            assert!(
                parse_reply(5, "kai.test", &r[..len]).is_err(),
                "cut at {len}"
            );
        }
        // A pointer that points forward, at itself.
        let mut looped = r.clone();
        let at = q.len();
        looped[at..at + 2].copy_from_slice(&[0xC0, at as u8]);
        assert_eq!(
            parse_reply(5, "kai.test", &looped),
            Err(ReplyError::Malformed)
        );
        // An A record whose data is not four bytes.
        let mut short = r;
        let len_at = short.len() - 6;
        short[len_at..len_at + 2].copy_from_slice(&3u16.to_be_bytes());
        short.pop();
        assert_eq!(
            parse_reply(5, "kai.test", &short),
            Err(ReplyError::Malformed)
        );
    }

    #[test]
    fn at_most_eight_addresses_are_kept() {
        let q = query(1, "many.test");
        let answers: Vec<_> = (0..12u8)
            .map(|i| ("many.test", 60, Data::A([10, 0, 0, i])))
            .collect();
        let answer = parse_reply(1, "many.test", &reply(&q, "many.test", 0, &answers)).unwrap();
        assert_eq!(answer.addresses.len(), MAX_ADDRESSES);
    }
}
