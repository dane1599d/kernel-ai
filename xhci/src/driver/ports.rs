//! Hot-plug on the root ports (P7 row 5). [`Controller::poll`] only marks a
//! port whose status changed; [`Controller::service_step`], called again
//! after the wait each step returns, then detaches what was on it and sets
//! up what is on it now. A detached device's slot is disabled and its pages kept for the next
//! device, since a `Dma` is never freed.
//!
//! A change behind a hub is not seen: the hub's status-change endpoint is not
//! listened to, so a device unplugged from a hub is detached only when its
//! hub's root port changes.

use super::enumerate::{HUB_BUDGET_NS, RESET_RECOVERY_NS, SETTLE_NS};
use super::{Controller, PortReport, RESET_TIMEOUT_NS};
use crate::Error;
use crate::context::Path;
use crate::hal::{Dma, Hal};
use crate::regs::port;
use crate::trb::{Event, Trb};

/// How often a port reset is looked at while it runs.
pub(super) const RESET_POLL_NS: u64 = 1_000_000;

/// Where servicing the root ports stands between two steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Service {
    Idle,
    /// Something was connected on `port`: it is set up if still there at
    /// `until`.
    Debounce {
        port: u8,
        until: u64,
    },
    /// `port` is being reset; given up after `until`.
    Resetting {
        port: u8,
        until: u64,
    },
    /// `port`'s reset is done; its recovery time ends at `until`.
    Recovering {
        port: u8,
        until: u64,
    },
}

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

    /// Services each root port marked as changed, waiting between steps
    /// (`hal.delay`): [`Self::service_step`] until it is done. For a caller
    /// that may hold the controller throughout; one that shares it with an
    /// interrupt runs the steps itself and lets go between them.
    pub fn service_ports(&mut self, mut report: impl FnMut(Change)) {
        while let Some(wait) = self.service_step(&mut report) {
            self.hal.delay(wait);
        }
    }

    /// Takes the next step in servicing the root ports marked as changed, and
    /// returns how long to wait before the next one, or `None` once every
    /// marked port is serviced.
    ///
    /// A connect change, or a port no longer connected or enabled, detaches
    /// every device on it, hubs and what is behind them; a connect change with
    /// something connected then sets that up once it is still there after the
    /// debounce and the port's reset, and arms its endpoints. A change of the
    /// driver's own making (a port reset) does neither.
    ///
    /// The debounce, the reset and its recovery are waited for between steps.
    /// A step itself runs commands and control transfers, which take
    /// microseconds; setting up a hub also waits inside its step, for its
    /// ports' power and resets. Other devices' reports that come during a
    /// step are kept for the next [`Self::poll`].
    pub fn service_step(&mut self, report: &mut impl FnMut(Change)) -> Option<u64> {
        let now = self.hal.now_ns();
        let left = |until: u64| until.saturating_sub(now);
        match self.service {
            Service::Idle => {
                let number = self.take_changed()?;
                let at = self.caps.portsc(number);
                let portsc = self.hal.read32(at);
                // Cleared first: a change after this read marks the port again.
                self.hal
                    .write32(at, port::write_value(portsc, portsc & port::CHANGES));
                let connect_change = portsc & port::CONNECT_CHANGE != 0;
                let up = portsc & (port::CONNECTED | port::ENABLED);
                if connect_change || up != port::CONNECTED | port::ENABLED {
                    self.detach_port(number, report);
                }
                if connect_change && portsc & port::CONNECTED != 0 {
                    // Debounce (USB 2.0 7.1.7.3): a device still there after
                    // the settle time is set up.
                    self.service = Service::Debounce {
                        port: number,
                        until: now.saturating_add(SETTLE_NS),
                    };
                    return Some(SETTLE_NS);
                }
                Some(0)
            }
            Service::Debounce { until, .. } if now < until => Some(left(until)),
            Service::Debounce { port: number, .. } => {
                let at = self.caps.portsc(number);
                let portsc = self.hal.read32(at);
                if portsc & port::CONNECTED == 0 {
                    self.service = Service::Idle;
                } else if portsc & port::ENABLED != 0 {
                    // A USB 3 port enables itself.
                    self.set_up_port(number, report);
                } else {
                    self.hal.write32(at, port::write_value(portsc, port::RESET));
                    self.service = Service::Resetting {
                        port: number,
                        until: now.saturating_add(RESET_TIMEOUT_NS),
                    };
                    return Some(RESET_POLL_NS);
                }
                Some(0)
            }
            Service::Resetting {
                port: number,
                until,
            } => {
                let at = self.caps.portsc(number);
                let portsc = self.hal.read32(at);
                if portsc & port::RESET_CHANGE != 0 {
                    self.hal
                        .write32(at, port::write_value(portsc, port::RESET_CHANGE));
                    self.service = Service::Recovering {
                        port: number,
                        until: now.saturating_add(RESET_RECOVERY_NS),
                    };
                    return Some(RESET_RECOVERY_NS);
                }
                if now <= until {
                    return Some(RESET_POLL_NS);
                }
                self.service = Service::Idle;
                report(Change::Attached(PortReport {
                    port: number,
                    route: 0,
                    outcome: Err(Error::Timeout),
                }));
                Some(0)
            }
            Service::Recovering { until, .. } if now < until => Some(left(until)),
            Service::Recovering { port: number, .. } => {
                self.set_up_port(number, report);
                Some(0)
            }
        }
    }

    /// Sets up the device on root port `number`, reset and enabled, and arms
    /// its endpoints.
    fn set_up_port(&mut self, number: u8, report: &mut impl FnMut(Change)) {
        self.service = Service::Idle;
        let portsc = self.hal.read32(self.caps.portsc(number));
        let before = self.endpoints.each_ref().map(Option::is_some);
        self.hub_deadline = self.hal.now_ns().saturating_add(HUB_BUDGET_NS);
        let path = Path {
            root_port: number,
            ..Path::default()
        };
        let mut attached = |r| report(Change::Attached(r));
        let outcome = match portsc & port::ENABLED {
            0 => Err(Error::Timeout),
            _ => self.attach(path, port::speed(portsc), 0, &mut attached),
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
