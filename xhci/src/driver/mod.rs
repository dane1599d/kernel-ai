//! The controller: bring-up, the command ring, control transfers and the event
//! ring, all waited on synchronously while devices are set up; afterwards
//! [`Controller::poll`] turns interrupt-IN completions into [`Input`].

mod enumerate;

use crate::hal::{Dma, Hal};
use crate::hid::{self, PointerLayout};
use crate::regs::{self, Capabilities, legacy};
use crate::ring::{self, Consumer, Producer};
use crate::trb::{self, Direction, Event, Trb};
use crate::{Error, PAGE};

pub use enumerate::{Attached, PortReport};

/// Devices and interrupt endpoints the driver keeps.
const MAX_DEVICES: usize = 8;
const MAX_ENDPOINTS: usize = 8;
const RESET_TIMEOUT_NS: u64 = 1_000_000_000;
const COMMAND_TIMEOUT_NS: u64 = 1_000_000_000;
const HANDOFF_TIMEOUT_NS: u64 = 1_000_000_000;
/// Events one poll handles at most, so a flood cannot hold the caller.
const POLL_LIMIT: usize = 64;
/// IMOD: at most one interrupt per 250 us (in 250 ns units).
const MODERATION: u32 = 1000;
/// Transfer errors in a row after which an endpoint is given up.
const MAX_FAILURES: u8 = 8;

/// What a HID interface turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    /// A relative pointer.
    Mouse,
    /// An absolute pointer.
    Tablet,
}

impl DeviceKind {
    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::Mouse => "mouse",
            DeviceKind::Tablet => "tablet",
        }
    }
}

/// One piece of input from a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A key, by its HID usage on the keyboard page, went down or up.
    Key { usage: u8, pressed: bool },
    /// A pointer report; `range` is set for a tablet's absolute position.
    Pointer {
        kind: DeviceKind,
        report: hid::Pointer,
    },
}

/// How an interrupt endpoint's reports are read.
#[derive(Clone, Copy, Debug)]
enum Decode {
    BootKeyboard(hid::Keyboard),
    BootMouse,
    Report(DeviceKind, PointerLayout),
}

/// One interrupt-IN endpoint with its ring and report buffer.
struct Endpoint {
    slot: u8,
    dci: u8,
    ring: Producer,
    ring_mem: Dma,
    /// The report buffer: an offset into its device's report page.
    device: usize,
    buffer: usize,
    len: u16,
    decode: Decode,
    /// A transfer is queued.
    armed: bool,
    /// Where getting the endpoint out of its halted state stands.
    recovery: Recovery,
    /// Transfer errors since the last good report.
    failures: u8,
}

/// An endpoint that halted on a transfer error is reset, then pointed past its
/// abandoned transfer, then armed again. Each step waits for its command's
/// completion in a later poll, so the poll never blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recovery {
    Idle,
    /// Reset Endpoint was queued; the command TRB's address.
    Resetting(u64),
    /// Set TR Dequeue Pointer was queued.
    Moving(u64),
}

/// The memory one device's slot uses for as long as the controller runs.
struct Device {
    slot: u8,
    _output: Dma,
    input: Dma,
    ep0: Producer,
    ep0_mem: Dma,
    /// Control transfers' data.
    data: Dma,
    /// Interrupt endpoints' reports.
    reports: Dma,
}

pub struct Controller<H: Hal> {
    hal: H,
    caps: Capabilities,
    dcbaa: Dma,
    commands: Producer,
    command_mem: Dma,
    events: Consumer,
    event_mem: Dma,
    erst: Dma,
    devices: [Option<Device>; MAX_DEVICES],
    endpoints: [Option<Endpoint>; MAX_ENDPOINTS],
    /// Transfers that ended in an error, since start.
    errors: u64,
}

impl<H: Hal> Controller<H> {
    /// Takes the controller from firmware, resets it, sets up its rings and runs
    /// it. Devices are set up by [`Self::enumerate`].
    pub fn start(mut hal: H) -> Result<Self, Error> {
        let caps = Capabilities::parse(|offset| hal.read32(offset));
        if !regs::takes_4k_pages(hal.read32(caps.op(regs::PAGESIZE))) {
            return Err(Error::PageSize);
        }
        let low = !caps.addr64;
        let page = |hal: &mut H| hal.dma(1, low).ok_or(Error::NoMemory);
        let dcbaa = page(&mut hal)?;
        let command_mem = page(&mut hal)?;
        let event_mem = page(&mut hal)?;
        let erst = page(&mut hal)?;
        let mut controller = Self {
            commands: Producer::new(command_mem.phys(), ring::ENTRIES),
            events: Consumer::new(event_mem.phys(), ring::ENTRIES),
            hal,
            caps,
            dcbaa,
            command_mem,
            event_mem,
            erst,
            devices: [const { None }; MAX_DEVICES],
            endpoints: [const { None }; MAX_ENDPOINTS],
            errors: 0,
        };
        controller.take_from_firmware();
        controller.reset()?;
        controller.set_up()?;
        Ok(controller)
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    fn op_read(&self, reg: usize) -> u32 {
        self.hal.read32(self.caps.op(reg))
    }

    fn op_write(&self, reg: usize, value: u32) {
        self.hal.write32(self.caps.op(reg), value)
    }

    fn write64(&self, offset: usize, value: u64) {
        self.hal.write32(offset, value as u32);
        self.hal.write32(offset + 4, (value >> 32) as u32);
    }

    fn wait(&self, timeout_ns: u64, mut done: impl FnMut(&Self) -> bool) -> Result<(), Error> {
        let start = self.hal.now_ns();
        loop {
            if done(self) {
                return Ok(());
            }
            if self.hal.now_ns().wrapping_sub(start) > timeout_ns {
                return Err(Error::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// Asks firmware to let go of the controller (the legacy support
    /// capability), and turns off its SMIs. Firmware that does not answer in
    /// time loses the controller anyway.
    fn take_from_firmware(&self) {
        let read = |at| self.hal.read32(at);
        let Some(at) = self
            .caps
            .extended
            .and_then(|first| regs::find_extended(first, legacy::ID, read))
        else {
            return;
        };
        let header = read(at);
        if header & legacy::BIOS_OWNED != 0 {
            self.hal.write32(at, header | legacy::OS_OWNED);
            let _ = self.wait(HANDOFF_TIMEOUT_NS, |c| {
                c.hal.read32(at) & legacy::BIOS_OWNED == 0
            });
        }
        let control = read(at + legacy::CONTROL);
        self.hal
            .write32(at + legacy::CONTROL, control & !legacy::SMI_ENABLES);
    }

    fn reset(&self) -> Result<(), Error> {
        self.wait(RESET_TIMEOUT_NS, |c| {
            c.op_read(regs::USBSTS) & regs::STS_NOT_READY == 0
        })?;
        let command = self.op_read(regs::USBCMD);
        if command & regs::CMD_RUN != 0 {
            self.op_write(regs::USBCMD, command & !regs::CMD_RUN);
        }
        self.wait(RESET_TIMEOUT_NS, |c| {
            c.op_read(regs::USBSTS) & regs::STS_HALTED != 0
        })?;
        self.op_write(regs::USBCMD, regs::CMD_RESET);
        self.wait(RESET_TIMEOUT_NS, |c| {
            c.op_read(regs::USBCMD) & regs::CMD_RESET == 0
                && c.op_read(regs::USBSTS) & regs::STS_NOT_READY == 0
        })
    }

    /// Points the controller at the slot array, the command ring and the event
    /// ring, and runs it.
    fn set_up(&mut self) -> Result<(), Error> {
        let caps = self.caps;
        self.op_write(regs::CONFIG, u32::from(caps.max_slots));
        if caps.scratchpads > 0 {
            let count = usize::from(caps.scratchpads);
            if count > PAGE / 8 {
                return Err(Error::TooMany);
            }
            let mut array = self.hal.dma(1, !caps.addr64).ok_or(Error::NoMemory)?;
            for index in 0..count {
                let page = self.hal.dma(1, !caps.addr64).ok_or(Error::NoMemory)?;
                // A Dma going out of scope is not freed: the controller owns the
                // scratchpad pages, and their array, from here on.
                array.write64(index * 8, page.phys());
            }
            self.dcbaa.write64(0, array.phys());
        }
        self.write64(caps.op(regs::DCBAAP), self.dcbaa.phys());
        self.write64(caps.op(regs::CRCR), self.commands.base() | regs::CRCR_CYCLE);
        let segment = ring::segment_entry(self.event_mem.phys(), ring::ENTRIES);
        self.erst.write_dwords(0, &segment);
        self.hal.write32(caps.rt(regs::ERSTSZ), 1);
        self.write64(caps.rt(regs::ERDP), self.events.erdp());
        self.write64(caps.rt(regs::ERSTBA), self.erst.phys());
        let mut command = regs::CMD_RUN;
        if self.hal.interrupts() {
            self.hal.write32(caps.rt(regs::IMOD), MODERATION);
            self.hal
                .write32(caps.rt(regs::IMAN), regs::IMAN_ENABLE | regs::IMAN_PENDING);
            command |= regs::CMD_INTERRUPTS;
        }
        self.op_write(regs::USBCMD, command);
        self.wait(RESET_TIMEOUT_NS, |c| {
            c.op_read(regs::USBSTS) & regs::STS_HALTED == 0
        })
    }

    fn ring_doorbell(&self, slot: u8, target: u8) {
        self.hal
            .write32(self.caps.doorbell(slot), u32::from(target));
    }

    /// Writes `trb` into the ring memory: the first three dwords, then the
    /// control dword whose cycle bit hands it to the controller.
    fn write_trb(mem: &mut Dma, write: ring::Write) {
        let at = usize::from(write.index) * trb::BYTES;
        mem.write_dwords(at, &write.trb.0[..3]);
        mem.write32(at + 12, write.trb.0[3]);
    }

    /// Queues `trb` on a ring and returns the TRB's physical address.
    fn queue(ring: &mut Producer, mem: &mut Dma, trb: Trb) -> u64 {
        let (write, link) = ring.push(trb);
        Self::write_trb(mem, write);
        if let Some(link) = link {
            Self::write_trb(mem, link);
        }
        ring.address(write.index)
    }

    /// The next event, if one is waiting; the dequeue pointer moves past it.
    fn next_event(&mut self) -> Option<Event> {
        let at = usize::from(self.events.index()) * trb::BYTES;
        let control = self.event_mem.read32(at + 12);
        if !self.events.is_new(&Trb([0, 0, 0, control])) {
            return None;
        }
        let event = Trb([
            self.event_mem.read32(at),
            self.event_mem.read32(at + 4),
            self.event_mem.read32(at + 8),
            control,
        ]);
        self.events.advance();
        Some(Event::parse(event))
    }

    fn update_dequeue(&self) {
        self.write64(self.caps.rt(regs::ERDP), self.events.erdp());
    }

    /// Waits for the event `wanted` picks out, dropping others (port changes,
    /// and nothing else while devices are being set up).
    fn wait_event(&mut self, mut wanted: impl FnMut(&Event) -> bool) -> Result<Event, Error> {
        let start = self.hal.now_ns();
        loop {
            if let Some(event) = self.next_event() {
                self.update_dequeue();
                if wanted(&event) {
                    return Ok(event);
                }
                continue;
            }
            if self.op_read(regs::USBSTS) & (regs::STS_HOST_ERROR | regs::STS_CONTROLLER_ERROR) != 0
            {
                return Err(Error::Halted);
            }
            if self.hal.now_ns().wrapping_sub(start) > COMMAND_TIMEOUT_NS {
                return Err(Error::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// Queues one command and rings for it; returns its TRB's address, which its
    /// completion event names.
    fn submit(&mut self, command: Trb) -> u64 {
        let address = Self::queue(&mut self.commands, &mut self.command_mem, command);
        self.ring_doorbell(0, 0);
        address
    }

    /// Runs one command and returns the slot its completion names.
    fn command(&mut self, command: Trb) -> Result<u8, Error> {
        let address = self.submit(command);
        let event =
            self.wait_event(|e| matches!(e, Event::Command { trb, .. } if *trb == address))?;
        match event {
            Event::Command { code, slot, .. } if code == trb::SUCCESS => Ok(slot),
            Event::Command { code, .. } => Err(Error::Completion(code)),
            _ => Err(Error::Timeout),
        }
    }

    /// Runs one control transfer on device `index`'s endpoint 0. A read lands in
    /// the device's data page; returns the bytes moved. A stall leaves the
    /// endpoint reset, ready for the next request.
    fn control(
        &mut self,
        index: usize,
        request: crate::descriptor::Request,
    ) -> Result<usize, Error> {
        use crate::descriptor::{length, reads};
        let device = self.devices[index].as_mut().ok_or(Error::TooMany)?;
        let len = length(&request).min(PAGE as u16);
        let direction = match (len, reads(&request)) {
            (0, _) => None,
            (_, true) => Some(Direction::In),
            (_, false) => Some(Direction::Out),
        };
        let slot = device.slot;
        Self::queue(
            &mut device.ep0,
            &mut device.ep0_mem,
            Trb::setup(request, direction),
        );
        let data = direction.map(|dir| {
            let trb = Trb::data(device.data.phys(), len, dir);
            Self::queue(&mut device.ep0, &mut device.ep0_mem, trb)
        });
        let status = Self::queue(&mut device.ep0, &mut device.ep0_mem, Trb::status(direction));
        self.ring_doorbell(slot, 1);
        let mut moved = usize::from(len);
        loop {
            let event = self.wait_event(
                |e| matches!(e, Event::Transfer { slot: s, dci: 1, .. } if *s == slot),
            )?;
            let Event::Transfer {
                trb, residue, code, ..
            } = event
            else {
                continue;
            };
            match code {
                trb::SHORT_PACKET if Some(trb) == data => {
                    moved = usize::from(len).saturating_sub(residue as usize);
                }
                trb::SUCCESS if trb == status => return Ok(moved),
                trb::SUCCESS => {}
                code => {
                    self.recover_ep0(index)?;
                    return Err(Error::Completion(code));
                }
            }
        }
    }

    /// Resets device `index`'s halted control endpoint and moves its dequeue
    /// pointer past what was queued.
    fn recover_ep0(&mut self, index: usize) -> Result<(), Error> {
        let device = self.devices[index].as_ref().ok_or(Error::TooMany)?;
        let (slot, (dequeue, cycle)) = (device.slot, device.ep0.dequeue());
        self.command(Trb::reset_endpoint(slot, 1))?;
        self.command(Trb::set_dequeue(slot, 1, dequeue, cycle))?;
        Ok(())
    }

    /// Queues the next report transfer on endpoint `index` and rings it.
    fn arm(&mut self, index: usize) {
        let Some(endpoint) = self.endpoints[index].as_mut() else {
            return;
        };
        let Some(device) = self.devices[endpoint.device].as_ref() else {
            return;
        };
        let buffer = device.reports.phys() + endpoint.buffer as u64;
        Self::queue(
            &mut endpoint.ring,
            &mut endpoint.ring_mem,
            Trb::normal(buffer, endpoint.len),
        );
        endpoint.armed = true;
        let (slot, dci) = (endpoint.slot, endpoint.dci);
        self.ring_doorbell(slot, dci);
    }

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
