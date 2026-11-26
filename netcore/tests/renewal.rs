//! `net-setup` failed on 2026-11-24 (batches #1304 and #1308, and #1119's
//! first run): its last ping to the gateway left at a DHCP renewal and got no
//! reply within its 1 s wait. The renewal's request to the server asked ARP
//! for the gateway; its answer brought the same address back, and rewriting
//! the address list emptied smoltcp's ARP cache inside the second smoltcp waits
//! before asking again. The ping then sent nothing for a second.
//!
//! Here smoltcp's own interface runs over a wire the test plays the gateway
//! on: the old rewrite stalls the next packet for that second, and
//! `netcore::address::put` keeps the gateway known instead.

use netcore::address::put;
use netcore::config::{FALLBACK, Static};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::icmp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Packet,
};
use std::collections::VecDeque;

const OURS: EthernetAddress = EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
const GATEWAY_MAC: EthernetAddress = EthernetAddress([0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const IDENT: u16 = 0x4B41;
const PAYLOAD: &[u8] = b"Kernel AI ping!!";

/// Frames the gateway sends us, and those we sent.
#[derive(Default)]
struct Wire {
    inbound: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
}

struct Rx(Vec<u8>);
struct Tx<'a>(&'a mut Vec<Vec<u8>>);

impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0; len];
        let result = f(&mut frame);
        self.0.push(frame);
        result
    }
}

impl Device for Wire {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        let frame = self.inbound.pop_front()?;
        Some((Rx(frame), Tx(&mut self.sent)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.sent))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

/// What left on the wire.
#[derive(Debug, PartialEq)]
enum Sent {
    ArpFor(Ipv4Address),
    Echo(u16),
}

/// The kernel's interface on QEMU's NAT network, with a ping socket.
struct Rig {
    wire: Wire,
    iface: Interface,
    sockets: SocketSet<'static>,
    ping: SocketHandle,
}

impl Rig {
    fn new() -> Self {
        let mut wire = Wire::default();
        let config = Config::new(HardwareAddress::Ethernet(OURS));
        let mut iface = Interface::new(config, &mut wire, Instant::from_millis(0));
        assert_eq!(put(&mut iface, FALLBACK), Ok(true));
        let buffer =
            || icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 4], vec![0; 1024]);
        let mut socket = icmp::Socket::new(buffer(), buffer());
        socket.bind(icmp::Endpoint::Ident(IDENT)).unwrap();
        let mut sockets = SocketSet::new(Vec::new());
        let ping = sockets.add(socket);
        Self {
            wire,
            iface,
            sockets,
            ping,
        }
    }

    fn echo(&mut self, seq: u16) {
        let request = Icmpv4Repr::EchoRequest {
            ident: IDENT,
            seq_no: seq,
            data: PAYLOAD,
        };
        let socket = self.sockets.get_mut::<icmp::Socket>(self.ping);
        let buffer = socket
            .send(request.buffer_len(), IpAddress::Ipv4(GATEWAY))
            .unwrap();
        request.emit(
            &mut Icmpv4Packet::new_unchecked(buffer),
            &ChecksumCapabilities::default(),
        );
    }

    /// The gateway answers an ARP request for itself.
    fn gateway_answers_arp(&mut self) {
        let arp = ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Reply,
            source_hardware_addr: GATEWAY_MAC,
            source_protocol_addr: GATEWAY,
            target_hardware_addr: OURS,
            target_protocol_addr: FALLBACK.address,
        };
        let mut frame = vec![0; EthernetFrame::<&[u8]>::header_len() + arp.buffer_len()];
        let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..]);
        ethernet.set_dst_addr(OURS);
        ethernet.set_src_addr(GATEWAY_MAC);
        ethernet.set_ethertype(EthernetProtocol::Arp);
        arp.emit(&mut ArpPacket::new_unchecked(ethernet.payload_mut()));
        self.wire.inbound.push_back(frame);
    }

    /// Polls at `ms` and says what left.
    fn poll(&mut self, ms: i64) -> Vec<Sent> {
        self.iface
            .poll(Instant::from_millis(ms), &mut self.wire, &mut self.sockets);
        self.wire.sent.drain(..).map(|frame| sent(&frame)).collect()
    }

    /// With an empty ARP cache, a packet to the gateway (the renewal's request
    /// to the DHCP server in `net-setup`) asks ARP at 1000 ms and leaves once
    /// it is answered: smoltcp asks for no neighbour again before 2000 ms.
    fn ask_arp_at_1000_ms(&mut self) {
        self.echo(0);
        assert_eq!(self.poll(1000), [Sent::ArpFor(GATEWAY)]);
        self.gateway_answers_arp();
        assert_eq!(self.poll(1002), [Sent::Echo(0)]);
    }
}

fn sent(frame: &[u8]) -> Sent {
    let ethernet = EthernetFrame::new_checked(frame).unwrap();
    match ethernet.ethertype() {
        EthernetProtocol::Arp => {
            let packet = ArpPacket::new_checked(ethernet.payload()).unwrap();
            match ArpRepr::parse(&packet).unwrap() {
                ArpRepr::EthernetIpv4 {
                    operation: ArpOperation::Request,
                    target_protocol_addr,
                    ..
                } => Sent::ArpFor(target_protocol_addr),
                other => panic!("not an ARP request: {other:?}"),
            }
        }
        EthernetProtocol::Ipv4 => {
            assert_eq!(ethernet.dst_addr(), GATEWAY_MAC);
            let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
            assert_eq!(ip.dst_addr(), GATEWAY);
            let icmp = Icmpv4Packet::new_checked(ip.payload()).unwrap();
            match Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()).unwrap() {
                Icmpv4Repr::EchoRequest { seq_no, .. } => Sent::Echo(seq_no),
                other => panic!("not an echo request: {other:?}"),
            }
        }
        other => panic!("unexpected frame: {other}"),
    }
}

#[test]
fn a_renewal_with_the_same_address_keeps_the_gateway_known() {
    let mut rig = Rig::new();
    rig.ask_arp_at_1000_ms();
    // The renewal's answer: the same address and gateway.
    assert_eq!(put(&mut rig.iface, FALLBACK), Ok(false));
    rig.echo(1);
    assert_eq!(rig.poll(1004), [Sent::Echo(1)]);
}

/// What the kernel did before: the rewrite inside that second leaves the next
/// packet with no ARP request and its socket waiting a second, longer than
/// `ping`'s 1000 ms wait for the reply.
#[test]
fn rewriting_the_same_address_in_that_second_stalls_the_next_packet() {
    let mut rig = Rig::new();
    rig.ask_arp_at_1000_ms();
    let cidr = IpCidr::new(IpAddress::Ipv4(FALLBACK.address), FALLBACK.prefix);
    rig.iface.update_ip_addrs(|addresses| {
        addresses.clear();
        addresses.push(cidr).unwrap();
    });
    rig.echo(1);
    for ms in 1004..2004 {
        assert_eq!(rig.poll(ms), [], "something left at {ms} ms");
    }
    assert_eq!(rig.poll(2004), [Sent::ArpFor(GATEWAY)]);
    rig.gateway_answers_arp();
    assert_eq!(rig.poll(2005), [Sent::Echo(1)]);
}

#[test]
fn a_new_address_or_gateway_is_put() {
    let mut rig = Rig::new();
    let moved = Static {
        address: Ipv4Address::new(10, 0, 2, 16),
        ..FALLBACK
    };
    assert_eq!(put(&mut rig.iface, moved), Ok(true));
    let cidr = IpCidr::new(IpAddress::Ipv4(moved.address), moved.prefix);
    assert_eq!(rig.iface.ip_addrs(), [cidr]);
    let rerouted = Static {
        gateway: Some(Ipv4Address::new(10, 0, 2, 1)),
        ..moved
    };
    assert_eq!(put(&mut rig.iface, rerouted), Ok(false));
    let route = rig.iface.routes().get_default_ipv4_route().unwrap();
    assert_eq!(route.via_router, IpAddress::v4(10, 0, 2, 1));
    let unrouted = Static {
        gateway: None,
        ..moved
    };
    assert_eq!(put(&mut rig.iface, unrouted), Ok(false));
    assert!(rig.iface.routes().get_default_ipv4_route().is_none());
}

/// A renewal that keeps the address and changes only its prefix is a change:
/// the list holds the address with its prefix, so it is rewritten.
#[test]
fn a_renewal_that_changes_only_the_prefix_rewrites_the_address() {
    let mut rig = Rig::new();
    let narrower = Static {
        prefix: 16,
        ..FALLBACK
    };
    assert_eq!(put(&mut rig.iface, narrower), Ok(true));
    let cidr = IpCidr::new(IpAddress::Ipv4(FALLBACK.address), 16);
    assert_eq!(rig.iface.ip_addrs(), [cidr]);
    // The same again changes nothing.
    assert_eq!(put(&mut rig.iface, narrower), Ok(false));
    // And back: the prefix alone is enough to rewrite it.
    assert_eq!(put(&mut rig.iface, FALLBACK), Ok(true));
    let cidr = IpCidr::new(IpAddress::Ipv4(FALLBACK.address), FALLBACK.prefix);
    assert_eq!(rig.iface.ip_addrs(), [cidr]);
}
