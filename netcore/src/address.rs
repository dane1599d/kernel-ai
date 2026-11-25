//! Putting an address setting on smoltcp's interface without losing its
//! neighbours.
//!
//! smoltcp empties its ARP cache whenever the address list is touched
//! (`Interface::update_ip_addrs`), but keeps the limit of one ARP request a
//! second it set when it last asked. A packet sent within that second to a
//! neighbour it has just forgotten sends no request, and its socket then waits
//! a second before trying again. A DHCP renewal does exactly that: the request
//! to the server asks ARP first, and the answer brings back the same address.
//! Rewriting it then left the next ping to the gateway unanswered for its whole
//! 1 s wait (`net-setup` on 2026-11-24, `tests/renewal.rs`). So the address
//! list is rewritten only when it changes.

use crate::config::Static;
use smoltcp::iface::Interface;
use smoltcp::wire::{IpAddress, IpCidr};

/// Gives `iface` the address and default route of `setting`. The address list
/// is rewritten, and the ARP cache with it, only when it differs; the route is
/// set each time, which forgets nothing. True when the address changed.
pub fn put(iface: &mut Interface, setting: Static) -> Result<bool, &'static str> {
    let cidr = IpCidr::new(IpAddress::Ipv4(setting.address), setting.prefix);
    let changed = iface.ip_addrs() != [cidr];
    if changed {
        let mut pushed = Ok(());
        iface.update_ip_addrs(|addresses| {
            addresses.clear();
            pushed = addresses.push(cidr).map_err(|_| "net: address table full");
        });
        pushed?;
    }
    iface.routes_mut().remove_default_ipv4_route();
    if let Some(gateway) = setting.gateway {
        iface
            .routes_mut()
            .add_default_ipv4_route(gateway)
            .map_err(|_| "net: route table full")?;
    }
    Ok(changed)
}
