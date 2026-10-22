//! After setup: [`Controller::poll`] turns finished interrupt-IN transfers into
//! [`Input`] and queues the next one, and gets an endpoint that halted on an
//! error going again without ever waiting.

use super::{Controller, Decode, DeviceKind, Input, MAX_FAILURES, POLL_LIMIT, Recovery};
use crate::hal::Hal;
use crate::hid;
use crate::regs;
use crate::trb::{self, Event, Trb};

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
        while handled < POLL_LIMIT {
            let Some(event) = self.next_event() else {
                break;
            };
            handled += 1;
            match event {
                Event::Transfer {
                    residue,
                    code,
                    slot,
                    dci,
                    ..
                } => self.on_transfer(slot, dci, code, residue, &mut out),
                Event::Command { trb, code, .. } => self.on_recovery_step(trb, code),
                _ => {}
            }
        }
        if handled > 0 {
            self.update_dequeue();
        }
        handled
    }

    fn on_transfer(
        &mut self,
        slot: u8,
        dci: u8,
        code: u8,
        residue: u32,
        out: &mut impl FnMut(Input),
    ) {
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
            self.errors += 1;
            endpoint.failures = endpoint.failures.saturating_add(1);
            if endpoint.failures <= MAX_FAILURES {
                let (slot, dci) = (endpoint.slot, endpoint.dci);
                let command = self.submit(Trb::reset_endpoint(slot, dci));
                if let Some(endpoint) = self.endpoints[index].as_mut() {
                    endpoint.recovery = Recovery::Resetting(command);
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
                matches!(e.recovery, Recovery::Resetting(at) | Recovery::Moving(at) if at == command)
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
            Recovery::Moving(_) => self.arm(index),
            Recovery::Idle => {}
        }
    }

    /// How many interrupt endpoints have a transfer queued: a device that
    /// stopped answering drops out.
    pub fn listening(&self) -> usize {
        self.endpoints.iter().flatten().filter(|e| e.armed).count()
    }
}
