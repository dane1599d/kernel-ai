//! A destination's text read as hostile (ADR 0001 D2, P5): the kernel parses a ring-3
//! caller's text with [`Dest::parse`] when it narrows a handle, and a package's `net`
//! line goes through it too. Cut, mutated or swept, the text is read or refused, never
//! a panic, within a fixed allocation; what reads prints back as text that reads as
//! the same destination; and `within`, the rule that a handle only shrinks, holds
//! when checked against what each destination actually allows.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use core::net::Ipv4Addr;
use hostile::XorShift;
use netdest::{Dest, MAX_TEXT, Protocol};

/// The most bytes one parse may hold at once. `parse` collects a field's `+` parts
/// before it counts them, so a 1 KiB run of `+` is the worst case: about a thousand
/// slices.
const PEAK_BYTES: usize = 32 * 1024;

const PROTOS: [&str; 3] = ["", "proto:tcp", "proto:udp"];
const ADDRS: [&str; 9] = [
    "",
    "addrs:any",
    "addrs:none",
    "addrs:10.0.0.0/8",
    "addrs:10.0.2.0/24",
    "addrs:10.0.2.15",
    "addrs:10.0.2.15+10.0.2.16",
    "addrs:0.0.0.0/0",
    "addrs:192.168.1.1/32",
];
const PORTS: [&str; 9] = [
    "",
    "ports:any",
    "ports:none",
    "ports:443",
    "ports:80+443",
    "ports:1-1024",
    "ports:443-443",
    "ports:0+65535",
    "ports:1024-65535",
];
const HOSTS: [&str; 8] = [
    "",
    "hosts:none",
    "hosts:any",
    "hosts:kai.test",
    "hosts:api.kai.test",
    "hosts:kai.test+example.org",
    "hosts:a.b.c.kai.test",
    "hosts:example.org",
];
const MATCHES: [&str; 3] = ["", "match:exact", "match:suffix"];

/// Destinations that parse, as the plan lines and the tests write them.
const SAMPLES: [&str; 5] = [
    "proto:tcp,addrs:10.0.2.0/24,ports:80+443,hosts:kai.test",
    "proto:udp,addrs:10.0.2.3,ports:53,hosts:none",
    "ports:443,hosts:api.weather.example+maps.example.org,match:exact",
    "proto:tcp,addrs:none,ports:443,hosts:any",
    "addrs:1.2.3.4+5.6.7.8+9.10.11.12,ports:1-65535,hosts:a-b.c-d.example",
];

/// A destination's text from one piece of each field, in a random order, with the
/// empty pieces left out.
fn random_text(rng: &mut XorShift) -> String {
    let mut fields: Vec<&str> = [
        PROTOS[rng.below(PROTOS.len())],
        ADDRS[rng.below(ADDRS.len())],
        PORTS[rng.below(PORTS.len())],
        HOSTS[rng.below(HOSTS.len())],
        MATCHES[rng.below(MATCHES.len())],
    ]
    .into_iter()
    .filter(|field| !field.is_empty())
    .collect();
    for at in (1..fields.len()).rev() {
        fields.swap(at, rng.below(at + 1));
    }
    fields.join(",")
}

/// What one input is held to: no panic, a bounded allocation, and a destination that
/// prints as text that reads back as it.
fn read(bytes: &[u8]) -> Option<Dest> {
    let text = core::str::from_utf8(bytes).ok()?;
    let (parsed, peak) = hostile::peak_alloc(|| Dest::parse(text));
    assert!(peak <= PEAK_BYTES, "{peak} bytes for {text:?}");
    let dest = parsed.ok()?;
    let printed = dest.to_string();
    match Dest::parse(&printed) {
        Ok(again) => assert_eq!(again, dest, "{text:?} printed as {printed:?}"),
        // The printed form names every field, so it can be longer than a compact
        // text that read; over the limit, the next read refuses it as too long
        // (kaiagent checks this length before it keeps a `net` line).
        Err(why) => assert!(
            printed.len() > MAX_TEXT,
            "{text:?} printed as {printed:?}, refused: {why}"
        ),
    }
    Some(dest)
}

#[test]
fn the_samples_and_every_cut_of_them_read_or_are_refused() {
    for sample in SAMPLES {
        assert!(read(sample.as_bytes()).is_some(), "{sample}");
        hostile::truncations(sample.as_bytes(), |_, cut| {
            read(cut);
        });
    }
}

#[test]
fn mutated_text_reads_or_is_refused_and_reads_back() {
    let mut read_ok = 0;
    for (seed, sample) in SAMPLES.iter().enumerate() {
        hostile::fuzz(
            sample.as_bytes(),
            0x4e37_0000 + seed as u64,
            20_000,
            |bytes| {
                read_ok += usize::from(read(bytes).is_some());
            },
        );
    }
    assert!(read_ok > 1_000, "only {read_ok} mutants read");
}

#[test]
fn random_field_combinations_read_or_are_refused_and_read_back() {
    let mut rng = XorShift::new(0x6e65_7464);
    for _ in 0..20_000 {
        read(random_text(&mut rng).as_bytes());
    }
}

#[test]
fn text_at_and_past_the_limits_is_refused_without_work() {
    let long = "a".repeat(MAX_TEXT + 1);
    let (parsed, peak) = hostile::peak_alloc(|| Dest::parse(&long));
    assert!(parsed.is_err());
    assert_eq!(peak, 0, "refused before anything is allocated");
    for text in [
        // Every part counted before the limit of 8 refuses it.
        format!("ports:{}", "+".repeat(MAX_TEXT - 6)),
        format!("addrs:{}", "1+".repeat((MAX_TEXT - 6) / 2)),
        format!("hosts:{}", "a.b+".repeat((MAX_TEXT - 6) / 4)),
        format!("ports:{}", "9".repeat(MAX_TEXT - 6)),
        format!("addrs:1.2.3.4/{}", "9".repeat(MAX_TEXT - 14)),
        ",".repeat(MAX_TEXT),
        ":".repeat(MAX_TEXT),
    ] {
        assert!(text.len() <= MAX_TEXT);
        assert!(read(text.as_bytes()).is_none(), "{text:.40}");
    }
    // A compact text that reads at the limit and prints longer than it.
    let label = "a".repeat(63);
    let host = format!("{label}.{label}.{label}.{}", "b".repeat(57));
    let hosts = [host.as_str(); 4].join("+");
    let text = format!("hosts:{hosts}");
    assert!(text.len() <= MAX_TEXT, "{}", text.len());
    let dest = read(text.as_bytes()).expect("reads at the limit");
    assert!(dest.to_string().len() > MAX_TEXT);
}

/// Addresses at and around the edges the samples' ranges have, and the ends.
fn probe_addrs(rng: &mut XorShift) -> Vec<Ipv4Addr> {
    let mut addrs: Vec<Ipv4Addr> = [
        "0.0.0.0",
        "255.255.255.255",
        "9.255.255.255",
        "10.0.0.0",
        "10.0.2.0",
        "10.0.2.14",
        "10.0.2.15",
        "10.0.2.16",
        "10.0.2.17",
        "10.0.2.255",
        "10.0.3.0",
        "10.255.255.255",
        "11.0.0.0",
        "192.168.1.0",
        "192.168.1.1",
        "192.168.1.2",
    ]
    .iter()
    .map(|a| a.parse().unwrap())
    .collect();
    addrs.extend((0..8).map(|_| Ipv4Addr::from(rng.next() as u32)));
    addrs
}

fn probe_ports(rng: &mut XorShift) -> Vec<u16> {
    let mut ports = vec![
        0, 1, 52, 53, 54, 79, 80, 81, 442, 443, 444, 1023, 1024, 1025, 65534, 65535,
    ];
    ports.extend((0..8).map(|_| rng.next() as u16));
    ports
}

const PROBE_NAMES: [&str; 16] = [
    "kai.test",
    "KAI.TEST.",
    "api.kai.test",
    "x.api.kai.test",
    "c.kai.test",
    "a.b.c.kai.test",
    "z.a.b.c.kai.test",
    "example.org",
    "www.example.org",
    "test",
    "evilkai.test",
    "kai.test.evil",
    "a.1",
    "10.0.2.15",
    "weather.example",
    "x--y.example.org",
];

/// The handle-only-shrinks rule, checked from outside: whenever `inner.within(outer)`,
/// every sampled name, address and port `inner` allows, `outer` allows too, on the
/// same protocol.
#[test]
fn a_destination_within_another_allows_nothing_the_other_does_not() {
    let mut rng = XorShift::new(0x7769_7468);
    let pool: Vec<Dest> = (0..2_000)
        .filter_map(|_| Dest::parse(&random_text(&mut rng)).ok())
        .collect();
    assert!(pool.len() > 1_000, "{}", pool.len());
    let mut within = 0;
    for _ in 0..20_000 {
        let inner = &pool[rng.below(pool.len())];
        let outer = &pool[rng.below(pool.len())];
        if !inner.within(outer) {
            continue;
        }
        within += 1;
        let says = || format!("{inner} is within {outer}");
        assert_eq!(inner.protocol, outer.protocol, "{}", says());
        let mut names: Vec<String> = PROBE_NAMES.iter().map(|n| n.to_string()).collect();
        for host in &inner.hosts {
            names.push(host.clone());
            names.push(format!("sub.{host}"));
        }
        for name in &names {
            if inner.allows_name(name) {
                assert!(outer.allows_name(name), "{}: {name}", says());
            }
        }
        let ports = probe_ports(&mut rng);
        for &port in &ports {
            for proto in [Protocol::Tcp, Protocol::Udp] {
                if inner.allows_resolved_port(proto, port) {
                    assert!(
                        outer.allows_resolved_port(proto, port),
                        "{}: {port}",
                        says()
                    );
                }
            }
        }
        for addr in probe_addrs(&mut rng) {
            for &port in &ports {
                if inner.allows_literal(inner.protocol, addr, port) {
                    assert!(
                        outer.allows_literal(inner.protocol, addr, port),
                        "{}: {addr}:{port}",
                        says()
                    );
                }
            }
        }
    }
    assert!(within > 1_000, "only {within} pairs were within");
}

/// `within` holds for a destination and itself, and for a copy narrowed one field at
/// a time, so the check above sees pairs that differ in one field as well as many.
#[test]
fn a_destination_is_within_itself_and_a_narrowed_copy_within_it() {
    let mut rng = XorShift::new(0x7365_6c66);
    for _ in 0..2_000 {
        let Ok(dest) = Dest::parse(&random_text(&mut rng)) else {
            continue;
        };
        assert!(dest.within(&dest), "{dest}");
        let no_ports = Dest {
            ports: netdest::PortRange::NoPorts,
            ..dest.clone()
        };
        let no_addrs = Dest {
            addrs: netdest::AddrRange::NoAddrs,
            ..dest.clone()
        };
        assert!(no_ports.within(&dest), "{no_ports} in {dest}");
        assert!(no_addrs.within(&dest), "{no_addrs} in {dest}");
        if !dest.any_host {
            let exact = Dest {
                exact: true,
                ..dest.clone()
            };
            assert!(exact.within(&dest), "{exact} in {dest}");
        }
    }
}
