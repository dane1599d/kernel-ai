//! The VM's network card, the HTTPS mock's forward, and the USB input devices.

use super::free_port;
use crate::cases::TestCase;
use std::path::Path;

/// The network card's `-device` value.
pub(super) const NET_DEVICE: &str = "virtio-net-pci,netdev=net0";
/// The user-mode network every VM gets.
pub(super) const USER_NET: &str = "user,id=net0";

/// Moves the network card behind a PCIe root port of its own (bus 1).
/// Forwards the guest's connections to `netcore::fixture::MOCK_ADDRESS` on
/// port 443 to the HTTPS mock on the host's `port` (P5 task 12's harness):
/// QEMU's user network starts `relay`, xtask's own `relay` subcommand, for each
/// connection, so every mock host a case names is reached on 443, as the
/// guest's TLS clients and `kai:net` require.
pub fn forward_https(args: &mut [String], relay: &Path, port: u16) {
    let [a, b, c, d] = netcore::fixture::MOCK_ADDRESS;
    if let Some(netdev) = args.iter_mut().find(|arg| arg.as_str() == USER_NET) {
        *netdev = format!(
            "{USER_NET},guestfwd=tcp:{a}.{b}.{c}.{d}:443-cmd:{} relay 127.0.0.1:{port}",
            command_word(&relay.display().to_string())
        );
    }
}

/// `XTASK_NETDEV`, when set, in place of the user network: the throughput
/// workflow's TAP device (`net-throughput`), since slirp's TCP to the guest
/// does not scale its window (spike P5-S2).
pub fn netdev_from_env(args: &mut [String]) {
    let Ok(netdev) = std::env::var("XTASK_NETDEV") else {
        return;
    };
    if let Some(arg) = args.iter_mut().find(|arg| arg.as_str() == USER_NET) {
        *arg = netdev;
    }
}

/// `word` as one word of a `cmd:` QEMU's user network splits as a shell
/// does, in single quotes, and with each comma doubled, as QEMU's option
/// parser reads a comma in a value (K's review of #753).
pub(super) fn command_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''")).replace(',', ",,")
}

/// Gives the network card no MSI-X vectors, so the kernel polls it.
pub fn net_without_vectors(args: &mut [String]) {
    if let Some(at) = args.iter().position(|arg| arg.starts_with(NET_DEVICE)) {
        args[at].push_str(",vectors=0");
    }
}

pub fn net_behind_bridge(args: &mut Vec<String>) {
    if let Some(at) = args.iter().position(|arg| arg == NET_DEVICE) {
        args[at] = format!("{NET_DEVICE},bus=kai-rp0");
        let port = [
            "pcie-root-port,id=kai-rp0,chassis=1".into(),
            "-device".into(),
        ];
        args.splice(at..at, port);
    }
}

/// The `qemu-xhci` controller with the case's USB devices, and a QMP socket
/// when the case sends `@` lines; the port goes to the test run.
pub fn input_args(case: &TestCase) -> Result<(Vec<String>, Option<u16>), String> {
    let mut args = Vec::new();
    if !case.usb.is_empty() {
        args.extend(["-device".into(), "qemu-xhci,id=xhci".into()]);
        for device in case.usb {
            args.extend(["-device".into(), format!("{device},bus=xhci.0")]);
        }
    }
    let wants_qmp = case
        .keys
        .iter()
        .flat_map(|stage| stage.iter())
        .any(|key| key.starts_with('@'));
    if !wants_qmp {
        return Ok((args, None));
    }
    let port = free_port()?;
    args.extend([
        "-qmp".into(),
        format!("tcp:127.0.0.1:{port},server=on,wait=off"),
    ]);
    Ok((args, Some(port)))
}
