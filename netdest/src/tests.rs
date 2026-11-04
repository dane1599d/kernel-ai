use super::*;
use std::string::ToString;

fn dest(text: &str) -> Dest {
    Dest::parse(text).unwrap()
}

const WIDE: &str = "proto:tcp,addrs:10.0.2.0/24,ports:80+443+8000+8001,hosts:kai.test+example.org";

#[test]
fn names_resolve_only_under_a_suffix() {
    let d = dest("proto:tcp,hosts:kai.test+example.org");
    assert!(d.allows_name("kai.test"));
    assert!(d.allows_name("a.b.kai.test"));
    assert!(d.allows_name("API.Example.ORG."));
    assert!(!d.allows_name("evilkai.test"));
    assert!(!d.allows_name("kai.test.evil.example"));
    assert!(!d.allows_name("secret-data.attacker.example"));
    assert!(!d.allows_name("10.0.2.99"));
    assert!(!d.allows_name(""));
    assert!(!d.allows_name("bad..kai.test"));
    assert!(!d.allows_name("kai.test.."));
}

#[test]
fn the_name_looked_up_is_the_name_checked() {
    assert_eq!(normalize("API.Kai.Test.").as_deref(), Some("api.kai.test"));
    assert_eq!(normalize("kai.test"), Some("kai.test".to_string()));
    assert_eq!(normalize("kai.test.."), None);
    assert_eq!(normalize("10.0.2.2"), None);
}

#[test]
fn a_field_left_out_allows_nothing() {
    let d = dest("hosts:kai.test");
    assert!(!d.allows_literal(Protocol::Tcp, Ipv4Addr::new(10, 0, 2, 2), 80));
    assert!(!d.allows_resolved_port(Protocol::Tcp, 80));
    assert!(dest("proto:tcp").hosts.is_empty());
}

#[test]
fn literal_connects_need_the_address_the_port_and_the_protocol() {
    let d = dest(WIDE);
    assert!(d.allows_literal(Protocol::Tcp, Ipv4Addr::new(10, 0, 2, 2), 443));
    assert!(d.allows_literal(Protocol::Tcp, Ipv4Addr::new(10, 0, 2, 255), 8001));
    assert!(!d.allows_literal(Protocol::Tcp, Ipv4Addr::new(10, 0, 3, 2), 443));
    assert!(!d.allows_literal(Protocol::Tcp, Ipv4Addr::new(10, 0, 2, 2), 22));
    assert!(!d.allows_literal(Protocol::Udp, Ipv4Addr::new(10, 0, 2, 2), 443));
}

#[test]
fn subnets_and_sets_match_as_in_tock() {
    let any = AddrRange::Subnet(Ipv4Addr::new(1, 2, 3, 4), 0);
    assert!(any.is_addr_valid(Ipv4Addr::new(255, 255, 255, 255)));
    let host = AddrRange::Subnet(Ipv4Addr::new(10, 0, 2, 2), 32);
    assert!(host.is_addr_valid(Ipv4Addr::new(10, 0, 2, 2)));
    assert!(!host.is_addr_valid(Ipv4Addr::new(10, 0, 2, 3)));
    let odd = AddrRange::Subnet(Ipv4Addr::new(10, 0, 2, 128), 25);
    assert!(odd.is_addr_valid(Ipv4Addr::new(10, 0, 2, 200)));
    assert!(!odd.is_addr_valid(Ipv4Addr::new(10, 0, 2, 127)));
    let set = AddrRange::AddrSet(std::vec![
        Ipv4Addr::new(1, 1, 1, 1),
        Ipv4Addr::new(9, 9, 9, 9)
    ]);
    assert!(set.is_addr_valid(Ipv4Addr::new(9, 9, 9, 9)));
    assert!(!AddrRange::NoAddrs.is_addr_valid(Ipv4Addr::new(1, 1, 1, 1)));
    assert!(PortRange::Range(10, 20).is_port_valid(20));
    assert!(!PortRange::Range(10, 20).is_port_valid(21));
    assert!(!PortRange::NoPorts.is_port_valid(0));
}

#[test]
fn a_handle_only_shrinks() {
    let wide = dest(WIDE);
    let narrow = dest("proto:tcp,addrs:10.0.2.2,ports:443,hosts:api.kai.test");
    assert!(narrow.within(&wide));
    assert!(!wide.within(&narrow));
    assert!(wide.within(&wide));
    // Each way of growing is refused.
    for grown in [
        "proto:udp,addrs:10.0.2.2,ports:443,hosts:api.kai.test",
        "proto:tcp,addrs:10.0.0.0/16,ports:443,hosts:api.kai.test",
        "proto:tcp,addrs:10.0.2.2,ports:22,hosts:api.kai.test",
        "proto:tcp,addrs:10.0.2.2,ports:443,hosts:kai.example",
        "proto:tcp,addrs:10.0.2.2,ports:443,hosts:evil.example",
        "proto:tcp,addrs:any,ports:443",
        "proto:tcp,ports:any",
    ] {
        assert!(!dest(grown).within(&wide), "{grown}");
    }
    // A union of the wide one's ports covers a range across them.
    let joined = dest("proto:tcp,ports:8000-8001");
    assert!(joined.within(&wide));
    let nothing = dest("proto:tcp");
    assert!(nothing.within(&wide));
}

#[test]
fn adjacent_spans_join_when_covering() {
    let outer = PortRange::PortSet(std::vec![1, 2, 3]);
    assert!(outer.covers(&PortRange::Range(1, 3)));
    assert!(!outer.covers(&PortRange::Range(1, 4)));
    let outer = AddrRange::AddrSet(std::vec![
        Ipv4Addr::new(10, 0, 0, 0),
        Ipv4Addr::new(10, 0, 0, 1)
    ]);
    assert!(outer.covers(&AddrRange::Subnet(Ipv4Addr::new(10, 0, 0, 0), 31)));
}

#[test]
fn the_text_form_reads_back() {
    for text in [
        WIDE,
        "proto:udp,addrs:any,ports:any,hosts:none",
        "proto:tcp,addrs:1.1.1.1+9.9.9.9,ports:53,hosts:a.example",
        "proto:tcp,addrs:none,ports:none,hosts:none",
    ] {
        let d = dest(text);
        assert_eq!(dest(&d.to_string()), d, "{text}");
    }
    assert_eq!(
        dest("hosts:Kai.Test").to_string(),
        "proto:tcp,addrs:none,ports:none,hosts:kai.test"
    );
}

#[test]
fn bad_text_is_refused() {
    for bad in [
        "",
        "proto:sctp",
        "proto:tcp,proto:udp",
        "colour:blue",
        "addrs:10.0.2.0/33",
        "addrs:10.0.2",
        "addrs:10.0.2.0/24+10.0.3.1",
        "addrs:1.1.1.1+1.1.1.2+1.1.1.3+1.1.1.4+1.1.1.5+1.1.1.6+1.1.1.7+1.1.1.8+1.1.1.9",
        "ports:443-80",
        "ports:65536",
        "hosts:com",
        "hosts:-a.example",
        "hosts:a..example",
        "hosts:10.0.2.2",
        "hosts:a_b.example",
        "nocolon",
    ] {
        assert!(Dest::parse(bad).is_err(), "{bad}");
    }
    assert!(Dest::parse(&"a".repeat(MAX_TEXT + 1)).is_err());
}
