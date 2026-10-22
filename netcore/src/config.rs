//! The network settings on the kernel command line.
//!
//! - `ip=dhcp` (the default) asks a DHCP server, and falls back to [`FALLBACK`]
//!   if none answers within `dhcp_wait_ms=` (default [`DHCP_WAIT_MS`]).
//! - `ip=static` uses [`FALLBACK`] at once; `ip=A.B.C.D/NN` (prefix 24 if left
//!   out) a static address of its own, with `gw=A.B.C.D` as its gateway.
//! - `dns=A.B.C.D[:PORT]` names the DNS server, whatever DHCP says.
//!
//! Two words exist for tests: `dhcp_port=N` sends DHCP to server port N (client
//! port N + 1), where no server listens, and `dhcp_max_lease_s=N` caps the lease
//! so renewal comes within seconds.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::net::Ipv4Addr;

/// The standard DNS port.
pub const DNS_PORT: u16 = 53;
/// How long DHCP has before the fallback address is used.
pub const DHCP_WAIT_MS: u64 = 5_000;
const MAX_DHCP_WAIT_MS: u64 = 600_000;

/// A static IPv4 setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Static {
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Option<Ipv4Addr>,
}

/// QEMU's and VirtualBox's NAT network: the guest, its gateway and DNS proxy.
pub const FALLBACK: Static = Static {
    address: Ipv4Addr::new(10, 0, 2, 15),
    prefix: 24,
    gateway: Some(Ipv4Addr::new(10, 0, 2, 2)),
};
/// The DNS server that goes with [`FALLBACK`].
pub const FALLBACK_DNS: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dhcp,
    Static(Static),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetConfig {
    pub mode: Mode,
    /// `dns=`: the server to ask instead of the one DHCP or the fallback names.
    pub dns: Option<(Ipv4Addr, u16)>,
    pub dhcp_wait_ms: u64,
    /// `dhcp_port=`: the DHCP server port, for the fallback test.
    pub dhcp_port: Option<u16>,
    /// `dhcp_max_lease_s=`: the longest lease to keep, for the renewal test.
    pub dhcp_max_lease_s: Option<u32>,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Dhcp,
            dns: None,
            dhcp_wait_ms: DHCP_WAIT_MS,
            dhcp_port: None,
            dhcp_max_lease_s: None,
        }
    }
}

impl NetConfig {
    /// The settings on `cmdline`, and a message for each word it could not use.
    pub fn parse(cmdline: &str) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut ignored = Vec::new();
        let mut address: Option<&str> = None;
        let mut gateway = None;
        for (key, value) in cmdline.split_whitespace().filter_map(|w| w.split_once('=')) {
            let bad = |what: &str| format!("net: ignored {key}={value}: {what}");
            match key {
                "ip" => address = Some(value),
                "gw" => match value.parse::<Ipv4Addr>() {
                    Ok(ip) => gateway = Some(ip),
                    Err(_) => ignored.push(bad("not an IPv4 address")),
                },
                "dns" => match endpoint(value, DNS_PORT) {
                    Some(server) => config.dns = Some(server),
                    None => ignored.push(bad("want IPv4[:PORT]")),
                },
                "dhcp_wait_ms" => match value.parse::<u64>() {
                    Ok(ms) if (1..=MAX_DHCP_WAIT_MS).contains(&ms) => config.dhcp_wait_ms = ms,
                    _ => ignored.push(bad("want 1 to 600000")),
                },
                "dhcp_port" => match value.parse::<u16>() {
                    Ok(port) if (1..u16::MAX).contains(&port) => config.dhcp_port = Some(port),
                    _ => ignored.push(bad("want a port below 65535")),
                },
                "dhcp_max_lease_s" => match value.parse::<u32>() {
                    Ok(seconds) if seconds >= 2 => config.dhcp_max_lease_s = Some(seconds),
                    _ => ignored.push(bad("want 2 or more")),
                },
                _ => {}
            }
        }
        match address {
            None | Some("dhcp") => {
                if gateway.is_some() {
                    ignored.push(String::from("net: ignored gw=: it needs a static ip="));
                }
            }
            Some("static") => {
                config.mode = Mode::Static(Static {
                    gateway: gateway.or(FALLBACK.gateway),
                    ..FALLBACK
                })
            }
            Some(text) => match cidr(text) {
                Some((address, prefix)) => {
                    config.mode = Mode::Static(Static {
                        address,
                        prefix,
                        gateway,
                    })
                }
                None => ignored.push(format!(
                    "net: ignored ip={text}: want dhcp, static or A.B.C.D/NN; using DHCP"
                )),
            },
        }
        (config, ignored)
    }
}

/// `A.B.C.D[:PORT]`, with `default_port` when the port is left out.
pub fn endpoint(text: &str, default_port: u16) -> Option<(Ipv4Addr, u16)> {
    let (host, port) = match text.split_once(':') {
        Some((host, port)) => (host, port.parse().ok().filter(|&p| p > 0)?),
        None => (text, default_port),
    };
    Some((host.parse().ok()?, port))
}

/// `A.B.C.D/NN`, or `A.B.C.D` as a /24. The address must be a usable host.
fn cidr(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address, prefix) = match text.split_once('/') {
        Some((address, prefix)) => (address, prefix.parse().ok()?),
        None => (text, 24),
    };
    let address: Ipv4Addr = address.parse().ok()?;
    let usable = !address.is_unspecified() && !address.is_broadcast() && !address.is_multicast();
    (usable && (1..=30).contains(&prefix)).then_some((address, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dhcp_is_the_default() {
        let (config, ignored) = NetConfig::parse("test=net llm=10.0.2.2:8080");
        assert_eq!(config, NetConfig::default());
        assert!(ignored.is_empty());
    }

    #[test]
    fn static_uses_the_nat_network_and_keeps_a_gateway_override() {
        let (config, _) = NetConfig::parse("ip=static");
        assert_eq!(config.mode, Mode::Static(FALLBACK));
        let (config, _) = NetConfig::parse("ip=static gw=10.0.2.1");
        let Mode::Static(setting) = config.mode else {
            panic!("not static")
        };
        assert_eq!(setting.gateway, Some(Ipv4Addr::new(10, 0, 2, 1)));
    }

    #[test]
    fn an_explicit_address_takes_its_prefix_and_gateway() {
        let (config, ignored) = NetConfig::parse("ip=192.168.1.20/16 gw=192.168.0.1");
        assert!(ignored.is_empty());
        assert_eq!(
            config.mode,
            Mode::Static(Static {
                address: Ipv4Addr::new(192, 168, 1, 20),
                prefix: 16,
                gateway: Some(Ipv4Addr::new(192, 168, 0, 1)),
            })
        );
        let (config, _) = NetConfig::parse("ip=10.1.2.3");
        assert!(matches!(
            config.mode,
            Mode::Static(Static {
                prefix: 24,
                gateway: None,
                ..
            })
        ));
    }

    #[test]
    fn bad_words_are_reported_and_leave_dhcp() {
        for word in [
            "ip=10.0.2",
            "ip=10.0.2.15/31",
            "ip=0.0.0.0/8",
            "ip=224.0.0.1/24",
        ] {
            let (config, ignored) = NetConfig::parse(word);
            assert_eq!(config.mode, Mode::Dhcp, "{word}");
            assert_eq!(ignored.len(), 1, "{word}");
        }
        let (_, ignored) = NetConfig::parse("gw=10.0.2.2");
        assert_eq!(ignored.len(), 1);
        let (config, ignored) =
            NetConfig::parse("dns=10.0.2.3:0 dhcp_wait_ms=0 dhcp_port=65535 dhcp_max_lease_s=1");
        assert_eq!(config, NetConfig::default());
        assert_eq!(ignored.len(), 4);
    }

    #[test]
    fn dns_and_test_words() {
        let (config, ignored) = NetConfig::parse(
            "dns=10.0.2.2:5353 dhcp_wait_ms=1500 dhcp_port=6767 dhcp_max_lease_s=6",
        );
        assert!(ignored.is_empty());
        assert_eq!(config.dns, Some((Ipv4Addr::new(10, 0, 2, 2), 5353)));
        assert_eq!(config.dhcp_wait_ms, 1500);
        assert_eq!(config.dhcp_port, Some(6767));
        assert_eq!(config.dhcp_max_lease_s, Some(6));
        let (config, _) = NetConfig::parse("dns=1.1.1.1");
        assert_eq!(config.dns, Some((Ipv4Addr::new(1, 1, 1, 1), DNS_PORT)));
    }
}
