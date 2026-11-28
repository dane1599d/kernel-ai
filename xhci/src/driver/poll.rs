//! After setup: [`Controller::poll`] turns finished interrupt-IN transfers into
//! [`Input`] and queues the next one, and gets an endpoint that halted on an
//! error going again without ever waiting.

use super::{Controller, Decode, DeviceKind, Input, MAX_FAILURES, POLL_LIMIT, Recovery};
use crate::hal::Hal;
use crate::hid;
use crate::regs;
use crate::trb::{self, Event, Trb};
use crate::{context, descriptor};

impl<H: Hal> Controller<H> {
    /// Handles the events waiting: each finished report is decoded into `out`,
    /// and its endpoint gets the next transfer queued. Returns how many events
    /// there were. With interrupts on, it also acknowledges the interrupt.
    pub fn poll(&mut self, mut out: impl FnMut(Input)) -> usize {
        if self.hal.interrupts() {
            let iman = self.caps.rt(regs::IMAN);
            self.hal
                .write32(iman, regs::IMAN_ENABLE | regs::IMAN_PENDING);
            self.op_write(regs::USBSTS, regs::STS_EVENT);
        }
        let mut handled = 0;
        // What came while something was waited for, in the order it came.
        for at in 0..self.deferred.len() {
            if let Some(event) = self.deferred[at].take() {
                handled += 1;
                self.handle(event, &mut out);
            }
        }
        let mut read = 0;
        while read < POLL_LIMIT {
            let Some(event) = self.next_event() else {
                break;
            };
            read += 1;
            self.handle(event, &mut out);
        }
        if read > 0 {
            self.update_dequeue();
        }
        handled + read
    }

    fn handle(&mut self, event: Event, out: &mut impl FnMut(Input)) {
        match event {
            Event::Transfer {
                trb,
                residue,
                code,
                slot,
                dci,
            } => self.on_transfer(slot, dci, trb, code, residue, out),
            Event::Command { trb, code, .. } => self.on_recovery_step(trb, code),
            Event::PortStatus { port } => self.mark(port),
            Event::Other { .. } => {}
        }
    }

    fn on_transfer(
        &mut self,
        slot: u8,
        dci: u8,
        trb: u64,
        code: u8,
        residue: u32,
        out: &mut impl FnMut(Input),
    ) {
        if dci == 1 {
            // After setup only a halt being cleared queues anything on EP0.
            self.on_clear_step(slot, trb, code);
            return;
        }
        let Some(index) = self
            .endpoints
            .iter()
            .position(|e| e.as_ref().is_some_and(|e| e.slot == slot && e.dci == dci))
        else {
            return;
        };
        let endpoint = self.endpoints[index].as_mut().expect("found just above");
        endpoint.armed = false;
        if code != trb::SUCCESS && code != trb::SHORT_PACKET {
            // The endpoint halted. A device that keeps failing is given up;
            // otherwise the endpoint is reset and armed again over later polls.
            // A transaction error is retried with the data toggle kept: reset,
            // the host would take the device's resent report for a repeat, ACK
            // it and drop it. Any other halt resets the toggle, and the device
            // is then told to clear its halt (and its toggle) too.
            self.errors += 1;
            endpoint.failures = endpoint.failures.saturating_add(1);
            if endpoint.failures <= MAX_FAILURES {
                let (slot, dci) = (endpoint.slot, endpoint.dci);
                let retry = code == trb::TRANSACTION_ERROR;
                let command = self.submit(match retry {
                    true => Trb::retry_endpoint(slot, dci),
                    false => Trb::reset_endpoint(slot, dci),
                });
                if let Some(endpoint) = self.endpoints[index].as_mut() {
                    endpoint.recovery = match retry {
                        true => Recovery::Retrying(command),
                        false => Recovery::Resetting(command),
                    };
                }
            }
            return;
        }
        endpoint.failures = 0;
        let len = usize::from(endpoint.len).saturating_sub(residue as usize);
        let mut report = [0u8; 64];
        let len = len.min(report.len());
        if let Some(device) = self.devices[endpoint.device].as_ref() {
            device
                .reports
                .read_bytes(endpoint.buffer, &mut report[..len]);
        }
        let report = &report[..len];
        match &mut endpoint.decode {
            Decode::BootKeyboard(keyboard) => {
                keyboard.report(report, |usage, pressed| out(Input::Key { usage, pressed }))
            }
            Decode::BootMouse => {
                if let Some(report) = hid::boot_mouse(report) {
                    out(Input::Pointer {
                        kind: DeviceKind::Mouse,
                        report,
                    });
                }
            }
            Decode::Report(kind, layout) => {
                if let Some(report) = layout.read(report) {
                    out(Input::Pointer {
                        kind: *kind,
                        report,
                    });
                }
            }
        }
        self.arm(index);
    }

    /// Takes the next step for the endpoint whose recovery command `command`
    /// just completed. A failed command leaves the endpoint quiet.
    fn on_recovery_step(&mut self, command: u64, code: u8) {
        let Some(index) = self.endpoints.iter().position(|e| {
            e.as_ref().is_some_and(|e| {
                matches!(
                    e.recovery,
                    Recovery::Resetting(at) | Recovery::Retrying(at) | Recovery::Moving(at)
                        if at == command
                )
            })
        }) else {
            return;
        };
        let endpoint = self.endpoints[index].as_mut().expect("found just above");
        let step = endpoint.recovery;
        endpoint.recovery = Recovery::Idle;
        if code != trb::SUCCESS {
            self.errors += 1;
            return;
        }
        match step {
            Recovery::Resetting(_) => {
                let (slot, dci) = (endpoint.slot, endpoint.dci);
                let (dequeue, cycle) = endpoint.ring.dequeue();
                let command = self.submit(Trb::set_dequeue(slot, dci, dequeue, cycle));
                if let Some(endpoint) = self.endpoints[index].as_mut() {
                    endpoint.recovery = Recovery::Moving(command);
                }
            }
            Recovery::Retrying(_) => {
                // The failed transfer is still queued where it stopped: ring
                // the endpoint to run it again.
                endpoint.armed = true;
                let (slot, dci) = (endpoint.slot, endpoint.dci);
                self.ring_doorbell(slot, dci);
            }
            Recovery::Moving(_) => self.clear_halt(index),
            Recovery::Clearing { .. } | Recovery::Idle => {}
        }
    }

    /// Endpoint `index`'s host side was reset to DATA0: asks the device, on its
    /// EP0 and without waiting, to clear the endpoint's halt, which ends a
    /// stall and resets the device's toggle to DATA0 too (USB 2.0 9.4.5). The
    /// endpoint is armed once the request's status stage completes.
    fn clear_halt(&mut self, index: usize) {
        let Some(endpoint) = self.endpoints[index].as_mut() else {
            return;
        };
        let Some(device) = self.devices[endpoint.device].as_mut() else {
            return;
        };
        let request = descriptor::clear_endpoint_halt(context::endpoint_address(endpoint.dci));
        let setup = Self::queue(
            &mut device.ep0,
            &mut device.ep0_mem,
            Trb::setup(request, None),
        );
        let status = Self::queue(&mut device.ep0, &mut device.ep0_mem, Trb::status(None));
        endpoint.recovery = Recovery::Clearing { setup, status };
        let slot = device.slot;
        self.ring_doorbell(slot, 1);
    }

    /// `slot`'s EP0 finished TRB `trb` of a halt being cleared: after the
    /// status stage the endpoint is armed; a refusal or an EP0 error leaves it
    /// quiet, as a failed recovery command does. A device whose EP0 halted
    /// here is not recovered: EP0 is not used again after setup.
    fn on_clear_step(&mut self, slot: u8, trb: u64, code: u8) {
        let Some(index) = self.endpoints.iter().position(|e| {
            e.as_ref().is_some_and(|e| {
                e.slot == slot
                    && matches!(
                        e.recovery,
                        Recovery::Clearing { setup, status } if trb == setup || trb == status
                    )
            })
        }) else {
            return;
        };
        let endpoint = self.endpoints[index].as_mut().expect("found just above");
        let Recovery::Clearing { status, .. } = endpoint.recovery else {
            return;
        };
        endpoint.recovery = Recovery::Idle;
        match code {
            trb::SUCCESS if trb == status => self.arm(index),
            _ => self.errors += 1,
        }
    }

    /// How many interrupt endpoints have a transfer queued: a device that
    /// stopped answering drops out.
    pub fn listening(&self) -> usize {
        self.endpoints.iter().flatten().filter(|e| e.armed).count()
    }
}
