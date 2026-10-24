//! `test=devices`: the device records (ADR 0007 section 3). The kernel's own
//! list holds the network card running and polled and the xHCI controller
//! running on MSI-X; then `handles mode=devlist` reads the same list from ring 3
//! through DEV_LIST and prints it.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{devices, log};
use kabi::device::{DeviceRecord, MAX_RECORDS, irq_mode, state};
use kabi::{ExitReason, Limits};

pub fn check() -> Result<(), &'static str> {
    let mut records = [DeviceRecord::default(); MAX_RECORDS];
    let count = devices::list(&mut records);
    let find = |driver: &str| records[..count].iter().find(|r| r.driver() == driver);
    let net = find("virtio-net").ok_or("devices: no record for the network card")?;
    if net.state != state::RUNNING || net.irq_mode != irq_mode::POLLED {
        return Err("devices: the network card is not running and polled");
    }
    let xhci = find("xhci").ok_or("devices: no record for the xHCI controller")?;
    if xhci.state != state::RUNNING || xhci.irq_mode != irq_mode::MSI_X {
        return Err("devices: the xHCI controller is not running on MSI-X");
    }
    if records[..count]
        .iter()
        .enumerate()
        .any(|(i, r)| usize::from(r.index) != i)
    {
        return Err("devices: the records are not numbered in order");
    }
    log!("devices: {count} records; virtio-net polled, xhci on MSI-X");
    let pid = process::spawn(&SpawnRequest {
        name: "devlist",
        program: "handles",
        args: b"mode=devlist",
        limits: Limits {
            max_pages: 256,
            max_burst_ms: 10_000,
            caps: 0,
            stack_pages: 0,
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "devices: cannot start handles mode=devlist")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => {
            log!("devices: DEV_LIST from ring 3 lists them too");
            Ok(())
        }
        _ => Err("devices: DEV_LIST from ring 3 failed"),
    }
}
