//! Hot-plug on the root ports (P7 row 5). [`Controller::poll`] only marks a
//! port whose status changed; [`Controller::service_ports`], which waits as
//! `enumerate` does, then detaches what was on it and sets up what is on it
//! now. A detached device's slot is disabled and its pages kept for the next
//! device, since a `Dma` is never freed.
//!
//! A change behind a hub is not seen: the hub's status-change endpoint is not
//! listened to, so a device unplugged from a hub is detached only when its
//! hub's root port changes.

use super::enumerate::{HUB_BUDGET_NS, SETTLE_NS};
use super::{Controller, PortReport};
use crate::context::Path;
use crate::hal::{Dma, Hal};
use crate::regs::port;
use crate::trb::{Event, Trb};

/// What [`Controller::service_ports`] did on a root port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The device at `port` and `route` (0 on the port itself) is gone: its
    /// slot is disabled and its endpoints dropped.
    Detached { port: u8, route: u32 },
    /// A device was connected, and set up or not, as `enumerate` reports one.
    Attached(PortReport),
}

impl<H: Hal> Controller<H> {
    /// Marks root port `port` as changed.
    pub(super) fn mark(&mut self, port: u8) {
        if (1..=self.caps.max_ports).contains(&port) {
            self.changed[usize::from(port / 64)] |= 1 << (port % 64);
        }
    }

    /// Whether a root port changed since the last [`Self::service_ports`].
    pub fn ports_changed(&self) -> bool {
        self.changed.iter().any(|word| *word != 0)
    }

    fn take_changed(&mut self) -> Option<u8> {
        let word = self.changed.iter().position(|word| *word != 0)?;
        let bit = self.changed[word].trailing_zeros();
        self.changed[word] &= !(1 << bit);
        Some((word * 64) as u8 + bit as u8)
    }

    /// Services each root port marked as changed. A connect change, or a port
    /// no longer connected or enabled, detaches every device on it, hubs and
    /// what is behind them; a connect change with something connected then
    /// sets that up, as `enumerate` does, and arms its endpoints. A change of
    /// the driver's own making (a port reset) does neither.
    ///
    /// It waits (port resets, settle times, commands), so it is called where
    /// waiting is allowed, never from the interrupt. Other devices' reports
    /// that come meanwhile are kept for the next [`Self::poll`].
    pub fn service_ports(&mut self, mut report: impl FnMut(Change)) {
        while let Some(number) = self.take_changed() {
            let at = self.caps.portsc(number);
            let portsc = self.hal.read32(at);
            // Cleared first: a change after this read marks the port again.
            self.hal
                .write32(at, port::write_value(portsc, portsc & port::CHANGES));
            let connect_change = portsc & port::CONNECT_CHANGE != 0;
            let up = portsc & (port::CONNECTED | port::ENABLED);
            if connect_change || up != port::CONNECTED | port::ENABLED {
                self.detach_port(number, &mut report);
            }
            if !connect_change || portsc & port::CONNECTED == 0 {
                continue;
            }
            // Debounce (USB 2.0 7.1.7.3): a device still there after the
            // settle time is set up.
            self.hal.delay(SETTLE_NS);
            if self.hal.read32(at) & port::CONNECTED == 0 {
                continue;
            }
            let before = self.endpoints.each_ref().map(Option::is_some);
            self.hub_deadline = self.hal.now_ns().saturating_add(HUB_BUDGET_NS);
            let path = Path {
                root_port: number,
                ..Path::default()
            };
            let mut attached = |r| report(Change::Attached(r));
            let outcome = match self.reset_port(number) {
                Ok(speed) => self.attach(path, speed, 0, &mut attached),
                Err(error) => Err(error),
            };
            attached(PortReport {
                port: number,
                route: 0,
                outcome,
            });
            for (index, was) in before.into_iter().enumerate() {
                if !was {
                    self.arm(index);
                }
            }
        }
    }

    /// Detaches every device on root port `number`.
    fn detach_port(&mut self, number: u8, report: &mut impl FnMut(Change)) {
        for index in 0..self.devices.len() {
            let Some(device) = self.devices[index].as_ref() else {
                continue;
            };
            if device.root_port != number {
                continue;
            }
            let route = device.route;
            self.detach(index);
            report(Change::Detached {
                port: number,
                route,
            });
        }
    }

    /// Disables device `index`'s slot and drops it and its endpoints. Their
    /// pages are kept for the next device once the controller has let go of
    /// them; if Disable Slot fails it may not have, and they are never used
    /// again.
    fn detach(&mut self, index: usize) {
        let Some(device) = self.devices[index].take() else {
            return;
        };
        let slot = device.slot;
        let freed = self.command(Trb::disable_slot(slot)).is_ok();
        if !freed {
            self.errors += 1;
        }
        self.dcbaa.write64(usize::from(slot) * 8, 0);
        // A report of the slot's that came before it was disabled is not for
        // whatever device gets the slot next.
        for kept in &mut self.deferred {
            if matches!(kept, Some(Event::Transfer { slot: s, .. }) if *s == slot) {
                *kept = None;
            }
        }
        for at in 0..self.endpoints.len() {
            if self.endpoints[at]
                .as_ref()
                .is_some_and(|e| e.device == index)
                && let Some(endpoint) = self.endpoints[at].take()
                && freed
            {
                self.give_back(endpoint.ring_mem);
            }
        }
        if freed {
            for page in [
                device.output,
                device.input,
                device.ep0_mem,
                device.data,
                device.reports,
            ] {
                self.give_back(page);
            }
        }
    }

    fn give_back(&mut self, page: Dma) {
        if let Some(free) = self.spare.iter_mut().find(|p| p.is_none()) {
            *free = Some(page);
        }
    }
}
