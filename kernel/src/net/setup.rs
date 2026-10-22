//! The interface's address: from DHCP by default, or static from the command line
//! (`netcore::config`). If DHCP is silent for `dhcp_wait_ms` the fallback, QEMU's
//! and VirtualBox's NAT address, is used while the client keeps asking; a lease
//! that comes later replaces it. smoltcp's client renews and rebinds the lease on
//! its own; a lease that runs out takes the address away, and the fallback timer
//! starts again.

use super::{NET, resolve};
use crate::{log, threads};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use core::net::Ipv4Addr;
use netcore::config::{FALLBACK, FALLBACK_DNS, Mode, NetConfig, Static};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::dhcpv4::{self, Event};
use smoltcp::time::Duration;
use smoltcp::wire::{DhcpPacket, DhcpRepr, IpAddress, IpCidr};
use spin::Once;

/// How often the interface is polled while it has no address.
pub const UNCONFIGURED_POLL_MS: u64 = 10;
/// How often the `net` thread polls once it has one: it answers ARP and keeps
/// the DHCP client's timers; anyone exchanging data polls far more often.
const IDLE_POLL_MS: u64 = 100;
/// Each DHCP packet received is copied here for its lease time.
const DHCP_PACKET_BUFFER: usize = 1500;
const MAX_DNS: usize = 3;

static CONFIG: Once<NetConfig> = Once::new();

/// Reads the network words of the kernel command line (see `netcore::config`).
pub fn configure(cmdline: &str) {
    let (config, ignored) = NetConfig::parse(cmdline);
    for why in ignored {
        log!("{why}");
    }
    CONFIG.call_once(|| config);
}

fn config() -> NetConfig {
    CONFIG.get().copied().unwrap_or_default()
}

/// Where the address came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Asking DHCP, with no address yet.
    Waiting,
    Dhcp {
        server: Ipv4Addr,
        /// The lease the server granted (capped by `dhcp_max_lease_s=`), if it
        /// said; None for an endless one.
        lease_s: Option<u32>,
        /// When it was granted, in HPET nanoseconds.
        since_ns: u64,
    },
    /// DHCP was silent: [`FALLBACK`], while the client keeps asking.
    Fallback,
    /// `ip=` on the command line.
    Static,
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub address: Option<(Ipv4Addr, u8)>,
    pub gateway: Option<Ipv4Addr>,
    /// The DNS servers DHCP or the fallback named; `dns=` overrides them.
    dns: [Option<Ipv4Addr>; MAX_DNS],
    pub source: Source,
    /// Leases taken (renewals that changed nothing are not counted).
    pub leases: u32,
    /// Leases that ran out.
    pub losses: u32,
}

impl Status {
    /// The DNS servers DHCP or the fallback named, before any `dns=`.
    pub fn named_dns(&self) -> [Option<Ipv4Addr>; MAX_DNS] {
        self.dns
    }

    /// The DNS servers to ask, in order, and whether `dns=` named them.
    pub fn dns_servers(&self) -> (impl Iterator<Item = (Ipv4Addr, u16)>, bool) {
        let forced = config().dns;
        let named = self
            .dns
            .into_iter()
            .flatten()
            .map(|ip| (ip, netcore::config::DNS_PORT));
        let list = forced
            .into_iter()
            .chain(named.filter(move |_| forced.is_none()));
        (list, forced.is_some())
    }
}

/// What a DHCP lease says.
struct Lease {
    address: (Ipv4Addr, u8),
    router: Option<Ipv4Addr>,
    dns: [Option<Ipv4Addr>; MAX_DNS],
    server: Ipv4Addr,
    lease_s: Option<u32>,
}

impl Lease {
    fn from(config: &dhcpv4::Config<'_>, cap_s: Option<u32>) -> Self {
        let mut dns = [None; MAX_DNS];
        for (slot, server) in dns.iter_mut().zip(config.dns_servers.iter()) {
            *slot = Some(*server);
        }
        let granted = config
            .packet
            .as_ref()
            .and_then(|packet: &DhcpPacket<&[u8]>| DhcpRepr::parse(packet).ok())
            .and_then(|repr| repr.lease_duration);
        let lease_s = match (granted, cap_s) {
            (Some(granted), Some(cap)) => Some(granted.min(cap)),
            (granted, None) => granted,
            (None, cap) => cap,
        };
        Self {
            address: (config.address.address(), config.address.prefix_len()),
            router: config.router,
            dns,
            server: config.server.address,
            lease_s,
        }
    }
}

pub struct Setup {
    dhcp: Option<SocketHandle>,
    status: Status,
    /// When the fallback is used if DHCP is still silent.
    fallback_at_ns: Option<u64>,
}

impl Setup {
    /// Applies a static address at once, or starts the DHCP client.
    pub fn start(
        iface: &mut Interface,
        sockets: &mut SocketSet<'static>,
        now_ns: u64,
    ) -> Result<Self, &'static str> {
        let config = config();
        let mut setup = Self {
            dhcp: None,
            status: Status {
                address: None,
                gateway: None,
                dns: [None; MAX_DNS],
                source: Source::Waiting,
                leases: 0,
                losses: 0,
            },
            fallback_at_ns: None,
        };
        match config.mode {
            Mode::Static(setting) => {
                // `ip=static` is the NAT network, whose DNS proxy is known.
                let dns = (setting.address == FALLBACK.address).then_some(FALLBACK_DNS);
                setup.apply(iface, setting, [dns, None, None], Source::Static)?;
            }
            Mode::Dhcp => {
                let mut socket = dhcpv4::Socket::new();
                if let Some(port) = config.dhcp_port {
                    socket.set_ports(port, port + 1);
                }
                if let Some(seconds) = config.dhcp_max_lease_s {
                    socket.set_max_lease_duration(Some(Duration::from_secs(seconds.into())));
                }
                let buffer = vec![0u8; DHCP_PACKET_BUFFER].into_boxed_slice();
                socket.set_receive_packet_buffer(Box::leak(buffer));
                setup.dhcp = Some(sockets.add(socket));
                setup.fallback_at_ns = Some(now_ns + config.dhcp_wait_ms * 1_000_000);
            }
        }
        Ok(setup)
    }

    /// How the setup began, for the boot log.
    pub fn describe_start(&self) -> String {
        match (self.status.source, self.status.address) {
            (Source::Static, Some((ip, prefix))) => {
                format!("static {ip}/{prefix}{}", via(self.status.gateway))
            }
            _ => format!(
                "asking DHCP (fallback {}/{} after {} ms)",
                FALLBACK.address,
                FALLBACK.prefix,
                config().dhcp_wait_ms
            ),
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// How long the `net` thread may sleep between polls.
    pub fn poll_gap_ms(&self) -> u64 {
        match self.status.address {
            None => UNCONFIGURED_POLL_MS,
            Some(_) => IDLE_POLL_MS,
        }
    }

    /// Takes the DHCP client's news after a poll, and uses the fallback once its
    /// time has come.
    pub fn after_poll(
        &mut self,
        iface: &mut Interface,
        sockets: &mut SocketSet<'static>,
        now_ns: u64,
    ) {
        if let Some(handle) = self.dhcp {
            let cap = config().dhcp_max_lease_s;
            let news = match sockets.get_mut::<dhcpv4::Socket>(handle).poll() {
                None => None,
                Some(Event::Configured(config)) => Some(Some(Lease::from(&config, cap))),
                Some(Event::Deconfigured) => Some(None),
            };
            match news {
                Some(Some(lease)) => self.take_lease(iface, lease, now_ns),
                Some(None) => self.lose_lease(iface, now_ns),
                None => {}
            }
        }
        if self.fallback_at_ns.is_some_and(|at| now_ns >= at) {
            self.fallback_at_ns = None;
            if self
                .apply(
                    iface,
                    FALLBACK,
                    [Some(FALLBACK_DNS), None, None],
                    Source::Fallback,
                )
                .is_ok()
            {
                log!(
                    "net: no DHCP answer in {} ms; using {}/{}{} while asking on",
                    config().dhcp_wait_ms,
                    FALLBACK.address,
                    FALLBACK.prefix,
                    via(FALLBACK.gateway)
                );
            }
        }
    }

    fn take_lease(&mut self, iface: &mut Interface, lease: Lease, now_ns: u64) {
        let was = self.status.source;
        let setting = Static {
            address: lease.address.0,
            prefix: lease.address.1,
            gateway: lease.router,
        };
        let source = Source::Dhcp {
            server: lease.server,
            lease_s: lease.lease_s,
            since_ns: now_ns,
        };
        if let Err(error) = self.apply(iface, setting, lease.dns, source) {
            log!("{error}");
            return;
        }
        self.fallback_at_ns = None;
        self.status.leases += 1;
        let lease_text = lease
            .lease_s
            .map_or_else(|| String::from("endless"), |s| netcore::duration(s.into()));
        let first_dns = lease.dns[0].map_or_else(|| String::from("none"), |ip| format!("{ip}"));
        log!(
            "net: DHCP lease {}/{}{} from {}, DNS {first_dns}, lease {lease_text}{}",
            setting.address,
            setting.prefix,
            via(setting.gateway),
            lease.server,
            match was {
                Source::Fallback => "; leaving the fallback",
                Source::Dhcp { .. } => "; the lease changed",
                _ => "",
            }
        );
    }

    fn lose_lease(&mut self, iface: &mut Interface, now_ns: u64) {
        if !matches!(self.status.source, Source::Dhcp { .. }) {
            return;
        }
        iface.update_ip_addrs(|addresses| addresses.clear());
        iface.routes_mut().remove_default_ipv4_route();
        self.status.address = None;
        self.status.gateway = None;
        self.status.dns = [None; MAX_DNS];
        self.status.source = Source::Waiting;
        self.status.losses += 1;
        self.fallback_at_ns = Some(now_ns + config().dhcp_wait_ms * 1_000_000);
        resolve::forget();
        log!("net: the DHCP lease ran out; asking again");
    }

    fn apply(
        &mut self,
        iface: &mut Interface,
        setting: Static,
        dns: [Option<Ipv4Addr>; MAX_DNS],
        source: Source,
    ) -> Result<(), &'static str> {
        let cidr = IpCidr::new(IpAddress::Ipv4(setting.address), setting.prefix);
        let mut pushed = Ok(());
        iface.update_ip_addrs(|addresses| {
            addresses.clear();
            pushed = addresses.push(cidr).map_err(|_| "net: address table full");
        });
        pushed?;
        iface.routes_mut().remove_default_ipv4_route();
        if let Some(gateway) = setting.gateway {
            iface
                .routes_mut()
                .add_default_ipv4_route(gateway)
                .map_err(|_| "net: route table full")?;
        }
        if self.status.address != Some((setting.address, setting.prefix)) || self.status.dns != dns
        {
            resolve::forget();
        }
        self.status.address = Some((setting.address, setting.prefix));
        self.status.gateway = setting.gateway;
        self.status.dns = dns;
        self.status.source = source;
        Ok(())
    }
}

/// ` via GATEWAY`, or nothing without one.
fn via(gateway: Option<Ipv4Addr>) -> String {
    gateway.map_or_else(String::new, |gw| format!(" via {gw}"))
}

/// The `net` thread: polls the interface between its users, so DHCP progresses
/// and ARP is answered when nobody is exchanging data.
pub fn run(_: usize) {
    loop {
        let gap = {
            let mut guard = NET.lock();
            let Some(net) = guard.as_mut() else {
                return;
            };
            net.poll();
            net.setup.poll_gap_ms()
        };
        threads::sleep_ms(gap);
    }
}
