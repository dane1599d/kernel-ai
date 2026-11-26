//! S07-2's case, `test=amdvi amdvi=on` (spike/amdvi-qemu only), on QEMU's
//! `amd-iommu` with virtio-net and virtio-rng at `iommu_platform=on` and a
//! keyboard and tablet on qemu-xhci:
//!
//! 1. virtio-net negotiated VIRTIO_F_ACCESS_PLATFORM, got a DHCP lease and
//!    pings the gateway, every DMA through the domain's table;
//! 2. qemu-xhci carries keys and a click (`usb::self_test`) the same way;
//! 3. no event was logged so far;
//! 4. virtio-rng fills a buffer (translated), then is handed the canary page:
//!    the IOMMU logs an IO_PAGE_FAULT for that device at that address, the
//!    canary is unchanged, and the device is reset with Bus Master off;
//! 5. the network still answers, so only that driver was stopped.

use crate::net::ping::{self, Outcome};
use crate::net::setup::Source;
use crate::virtio::hal::KernelHal;
use crate::{amdvi, hpet, log, memory, net, threads, usb};
use bitflags::bitflags;
use core::net::Ipv4Addr;
use spike_s07_2::{Event, IO_PAGE_FAULT};
use virtio_drivers::queue::VirtQueue;
use virtio_drivers::transport::{DeviceStatus, Transport};

const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
const PINGS: u16 = 3;
const SETUP_WAIT_MS: u64 = 20_000;
const FAULT_WAIT_MS: u64 = 3_000;
/// QEMU's IOMMU_WO in an IO_PAGE_FAULT's flags (amd_iommu.c passes its
/// access bits there).
const QEMU_WRITE: u16 = 0x2;

bitflags! {
    /// The rng's features the spike accepts: version 1, and the IOMMU's.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct RngFeatures: u64 {
        const VERSION_1 = 1 << 32;
        const ACCESS_PLATFORM = 1 << 33;
    }
}

pub fn check() -> Result<(), &'static str> {
    if !amdvi::active() {
        return Err("amdvi: the IOMMU was not set up");
    }
    let confined = amdvi::confined();
    for driver in ["virtio-net", "virtio-rng", "xhci"] {
        if !confined.iter().any(|c| c.driver == driver) {
            log!("amdvi: no {driver} in the domain");
            return Err("amdvi: a device the case needs is not in the domain");
        }
    }
    if !crate::virtio::split::access_platform_taken() {
        return Err("amdvi: virtio-net did not negotiate VIRTIO_F_ACCESS_PLATFORM");
    }
    wait_for_lease()?;
    pings("before the fault")?;
    log!(
        "amdvi: virtio-net DMA is translated: ACCESS_PLATFORM, a DHCP lease, {PINGS} of {PINGS} pings"
    );
    usb::self_test()?;
    log!("amdvi: qemu-xhci DMA is translated: keys and a click over USB");
    let (early, status) = amdvi::events();
    if !early.is_empty() {
        for event in &early {
            log_event(event);
        }
        return Err("amdvi: events were logged before the bad DMA");
    }
    log!("amdvi: no events before the bad DMA, status {status:#x}");
    rng_check()?;
    pings("after the fault")?;
    log!("amdvi: after the fault the network still answers: only the rng was stopped");
    log!("amdvi: S07-2 passed");
    Ok(())
}

fn log_event(event: &Event) {
    log!(
        "amdvi: event {} device {:#06x} address {:#x} flags {:#05x}",
        event.name(),
        event.device,
        event.address,
        event.flags
    );
}

fn wait_for_lease() -> Result<(), &'static str> {
    for _ in 0..SETUP_WAIT_MS / 10 {
        if net::status().is_some_and(|s| matches!(s.source, Source::Dhcp { .. })) {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    log!("amdvi: net setup never got there: {:?}", net::status());
    Err("amdvi: no DHCP lease through the IOMMU")
}

fn pings(when: &str) -> Result<(), &'static str> {
    let mut answered = 0;
    let stats = ping::ping(
        GATEWAY,
        PINGS,
        |_, outcome| answered += u16::from(matches!(outcome, Outcome::Reply(_))),
        || false,
    )?;
    log!(
        "amdvi: {when}: {answered} of {PINGS} pings answered ({})",
        stats.summary()
    );
    if answered != PINGS {
        return Err("amdvi: the gateway did not answer every ping");
    }
    Ok(())
}

fn wait(mut done: impl FnMut() -> bool, ms: u64) -> bool {
    let until = hpet::nanos() + ms * 1_000_000;
    while hpet::nanos() < until {
        if done() {
            return true;
        }
        threads::sleep_ms(1);
    }
    done()
}

fn rng_check() -> Result<(), &'static str> {
    let rng = amdvi::confined()
        .into_iter()
        .find(|c| c.driver == "virtio-rng")
        .ok_or("amdvi: no virtio-rng in the domain")?;
    let canary = amdvi::canary().ok_or("amdvi: no canary")?;
    let mut transport = amdvi::take_rng().ok_or("amdvi: the probe kept no virtio-rng")?;
    let features = transport.begin_init(RngFeatures::VERSION_1 | RngFeatures::ACCESS_PLATFORM);
    if !features.contains(RngFeatures::ACCESS_PLATFORM)
        || !transport.get_status().contains(DeviceStatus::FEATURES_OK)
    {
        return Err("amdvi: virtio-rng did not take VIRTIO_F_ACCESS_PLATFORM");
    }
    let mut queue = VirtQueue::<KernelHal, 4>::new(&mut transport, 0, false, false)
        .map_err(|_| "amdvi: the rng's queue did not set up")?;
    transport.finish_init();
    // A buffer on the stack is bounced through frames the table maps.
    let mut good = [0u8; 64];
    // SAFETY: `good` outlives the request, which is popped before it is read.
    let token = unsafe { queue.add(&[], &mut [&mut good[..]]) }
        .map_err(|_| "amdvi: the rng refused a request")?;
    transport.notify(0);
    if !wait(|| queue.can_pop(), 2_000) {
        return Err("amdvi: virtio-rng did not answer through the IOMMU");
    }
    // SAFETY: the same buffer as `add` was given.
    unsafe { queue.pop_used(token, &[], &mut [&mut good[..]]) }
        .map_err(|_| "amdvi: the rng's answer did not pop")?;
    if good.iter().all(|&b| b == 0) {
        return Err("amdvi: virtio-rng returned only zeros");
    }
    log!("amdvi: virtio-rng DMA is translated: 64 random bytes");
    // The canary page lies in the direct map, so the HAL hands the device its
    // own physical address: one the domain's table does not map.
    // SAFETY: the canary is ours, 4 KiB in the direct map; the device never
    // gets to write it, and the request is never popped.
    let page =
        unsafe { core::slice::from_raw_parts_mut((memory::hhdm() + canary) as *mut u8, 4096) };
    // SAFETY: as above.
    unsafe { queue.add(&[], &mut [page]) }.map_err(|_| "amdvi: the rng refused the canary")?;
    transport.notify(0);
    let mut seen = alloc::vec::Vec::new();
    let mut faulted = None;
    wait(
        || {
            let (events, _) = amdvi::events();
            for event in events {
                if seen.len() < 3 {
                    log_event(&event);
                }
                // QEMU 11.0.5 logs every event's address as 0: its
                // amdvi_setevent_bits masks the second quadword by the
                // absolute bit 64, not the bit in that word. Its flags are
                // QEMU's access bits, 0x2 a write, with the I bit (0x8).
                let at_canary = event.address & !0xFFF == canary || event.address == 0;
                let hit = event.code == IO_PAGE_FAULT
                    && event.device == rng.id
                    && at_canary
                    && event.flags & QEMU_WRITE != 0;
                if hit && faulted.is_none() {
                    faulted = Some(event);
                }
                seen.push(event);
            }
            faulted.is_some()
        },
        FAULT_WAIT_MS,
    );
    let answered = queue.can_pop();
    let intact = amdvi::canary_intact();
    // Stop the driver: reset the device, then take its bus mastering away.
    transport.set_status(DeviceStatus::empty());
    let off = amdvi::bus_master_off(rng.at);
    // The queue's memory stays with the reset device: a spike leaks a page.
    core::mem::forget(queue);
    log!(
        "amdvi: canary DMA: {} events, completed {answered}, canary intact {intact}, \
         reset with bus master off {off}",
        seen.len()
    );
    let Some(fault) = faulted else {
        return Err("amdvi: no IO_PAGE_FAULT for the rng at the canary");
    };
    if !intact {
        return Err("amdvi: the canary page changed");
    }
    if !off {
        return Err("amdvi: the rng's bus master did not clear");
    }
    log!(
        "amdvi: virtio-rng DMA to the canary at {canary:#x} faulted: IO_PAGE_FAULT for device \
         {:#06x}, a write, logged address {:#x}, flags {:#05x}, {} events; canary intact, the \
         device stopped",
        fault.device,
        fault.address,
        fault.flags,
        seen.len()
    );
    Ok(())
}
