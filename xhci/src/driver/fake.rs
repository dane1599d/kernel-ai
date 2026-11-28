//! A fake xHCI controller with USB devices on its root ports, behind the
//! driver's `Hal`: it keeps the registers, runs the command ring and each slot's
//! control endpoint when a doorbell is rung, and writes the completion events,
//! so `Controller::start` and `enumerate` run over it as over hardware. The
//! devices are keyboards, mice and USB 2 hubs, with the ports and requests a hub
//! has (USB 2.0 11.24). It checks nothing itself: it records what the driver put
//! in each slot context, for the tests to read.

use crate::hal::{Dma, Hal};
use crate::regs::{self, port, speed};
use crate::trb;
use std::boxed::Box;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

const OPERATIONAL: usize = 0x20;
const DOORBELLS: usize = 0x800;
const RUNTIME: usize = 0x600;
const SLOTS: u8 = 16;
/// Where the port registers start in the operational registers (xHCI 5.4).
const PORTS: usize = 0x400;

#[repr(C, align(4096))]
struct Page([u8; 4096]);

/// What a fake device is.
#[derive(Clone)]
pub enum Kind {
    Keyboard,
    Mouse,
    /// A USB 2 hub, with what is on each of its ports.
    Hub(Vec<Option<Device>>),
}

/// One fake device, and the hub port state that goes with it when it sits on one.
#[derive(Clone)]
pub struct Device {
    pub kind: Kind,
    pub speed: u8,
    /// A SuperSpeed hub: its bcdUSB says 3.0.
    pub super_speed: bool,
    /// Resets of its port never finish.
    pub stuck: bool,
    powered: bool,
    enabled: bool,
    change: u16,
}

impl Device {
    fn new(kind: Kind, speed: u8) -> Self {
        Self {
            kind,
            speed,
            super_speed: false,
            stuck: false,
            powered: false,
            enabled: false,
            change: 0,
        }
    }

    pub fn keyboard() -> Self {
        Self::new(Kind::Keyboard, speed::FULL)
    }

    pub fn mouse() -> Self {
        Self::new(Kind::Mouse, speed::LOW)
    }

    /// A high-speed hub.
    pub fn hub(ports: Vec<Option<Device>>) -> Self {
        Self::new(Kind::Hub(ports), speed::HIGH)
    }

    /// A full-speed hub.
    pub fn slow_hub(ports: Vec<Option<Device>>) -> Self {
        Self::new(Kind::Hub(ports), speed::FULL)
    }

    pub fn super_hub(ports: Vec<Option<Device>>) -> Self {
        let mut hub = Self::new(Kind::Hub(ports), speed::SUPER);
        hub.super_speed = true;
        hub
    }

    fn descriptor(&self) -> [u8; 18] {
        let class = matches!(self.kind, Kind::Hub(_)).then_some(9).unwrap_or(0);
        let usb = if self.super_speed { 0x0300u16 } else { 0x0200 };
        let [u0, u1] = usb.to_le_bytes();
        let packet = if self.super_speed { 9 } else { 64 };
        [
            18, 1, u0, u1, class, 0, 0, packet, 0x27, 0x06, 0x01, 0x00, 0, 1, 0, 0, 0, 1,
        ]
    }

    fn configuration(&self) -> Vec<u8> {
        let (class, subclass, protocol, extra): (u8, u8, u8, &[u8]) = match self.kind {
            Kind::Keyboard => (3, 1, 1, &[9, 0x21, 0x11, 1, 0, 1, 0x22, 63, 0]),
            Kind::Mouse => (3, 1, 2, &[9, 0x21, 0x11, 1, 0, 1, 0x22, 0, 0]),
            Kind::Hub(_) => (9, 0, 0, &[]),
        };
        let mut bytes = vec![9, 2, 0, 0, 1, 1, 0, 0xA0, 50];
        bytes.extend([9, 4, 0, 0, 1, class, subclass, protocol, 0]);
        bytes.extend(extra);
        bytes.extend([7, 5, 0x81, 3, 8, 0, 10]);
        let total = bytes.len() as u16;
        bytes[2..4].copy_from_slice(&total.to_le_bytes());
        bytes
    }

    /// The device at the end of `route` below this one (a hub's ports, the
    /// lowest nibble first).
    fn at(&mut self, route: u32) -> Option<&mut Device> {
        if route == 0 {
            return Some(self);
        }
        let Kind::Hub(ports) = &mut self.kind else {
            return None;
        };
        let port = (route & 0xF) as usize;
        let behind = ports.get_mut(port.checked_sub(1)?)?.as_mut()?;
        // Nothing is reached through a port that is not powered and enabled.
        if !(behind.powered && behind.enabled) {
            return None;
        }
        behind.at(route >> 4)
    }

    /// A control request to this device; None is a stall.
    fn control(&mut self, setup: [u8; 8]) -> Option<Vec<u8>> {
        let length = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = usize::from(u16::from_le_bytes([setup[4], setup[5]]));
        let mut answer = match (setup[0], setup[1]) {
            (0x80, 6) => match (value >> 8) as u8 {
                1 => self.descriptor().to_vec(),
                2 => self.configuration(),
                _ => return None,
            },
            (0x00, 9) | (0x21, 0x0A) | (0x21, 0x0B) => Vec::new(),
            (0xA0, 6) => {
                let Kind::Hub(ports) = &self.kind else {
                    return None;
                };
                // As QEMU's hub sends it: characteristics 0x000a (a think time of
                // 8 bit times), 2 ms to power good, and a bLength of 7 plus the
                // two masks, which is 10 for 8 ports and more.
                let masks = (ports.len() + 1).div_ceil(8);
                let mut bytes = vec![0, 0x29, ports.len() as u8, 0x0A, 0, 1, 0];
                bytes.extend(vec![0xFF; 2 * masks]);
                bytes[0] = bytes.len() as u8;
                bytes
            }
            (0xA3, 0) => {
                let port = self.port(index)?;
                let mut status = 0u16;
                let Some(device) = port.as_ref().filter(|d| d.powered) else {
                    return Some(vec![0x00, 0x01, 0, 0]);
                };
                status |= 1 | (1 << 8);
                status |= u16::from(device.enabled) << 1;
                status |= u16::from(device.speed == speed::LOW) << 9;
                status |= u16::from(device.speed == speed::HIGH) << 10;
                let [s0, s1] = status.to_le_bytes();
                let [c0, c1] = device.change.to_le_bytes();
                vec![s0, s1, c0, c1]
            }
            (0x23, 3) => {
                if let Some(Some(device)) = self.port(index) {
                    match value {
                        8 => {
                            device.powered = true;
                            device.change |= 1;
                        }
                        4 if device.powered && !device.stuck => {
                            device.enabled = true;
                            device.change |= 1 << 4;
                        }
                        _ => {}
                    }
                } else if self.port(index).is_none() {
                    return None;
                }
                Vec::new()
            }
            (0x23, 1) => {
                if let Some(Some(device)) = self.port(index) {
                    match value {
                        16 => device.change &= !1,
                        20 => device.change &= !(1 << 4),
                        _ => {}
                    }
                } else if self.port(index).is_none() {
                    return None;
                }
                Vec::new()
            }
            _ => return None,
        };
        answer.truncate(length);
        Some(answer)
    }

    /// Port `number` (1-based) of a hub: None when this is no hub or has no such
    /// port, Some of its device, if one is on it.
    fn port(&mut self, number: usize) -> Option<&mut Option<Device>> {
        let Kind::Hub(ports) = &mut self.kind else {
            return None;
        };
        ports.get_mut(number.checked_sub(1)?)
    }
}

/// What the driver put in a slot's contexts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotRecord {
    pub speed: u8,
    pub route: u32,
    pub root_port: u8,
    pub hub: bool,
    pub hub_ports: u8,
    pub think_time: u8,
    pub parent_slot: u8,
    pub parent_port: u8,
    /// Whether Address Device has run, and Configure Endpoint after it.
    pub addressed: bool,
    pub configures: u32,
    /// The slot context's first four dwords, as Address Device read them.
    pub raw: [u32; 4],
}

struct Slot {
    record: SlotRecord,
    ep0: u64,
    cycle: bool,
    /// Between Enable Slot and Disable Slot.
    enabled: bool,
    /// Interrupt endpoints' transfer rings, by DCI: where the next TRB is.
    rings: BTreeMap<u8, (u64, bool)>,
}

impl Slot {
    fn new() -> Self {
        Self {
            record: SlotRecord::default(),
            ep0: 0,
            cycle: true,
            enabled: true,
            rings: BTreeMap::new(),
        }
    }
}

struct Xhc {
    ports: Vec<Option<Device>>,
    root: Vec<PortRegs>,
    pages: BTreeMap<u64, *mut u8>,
    next_page: u64,
    usbcmd: u32,
    dcbaap: u64,
    crcr: u64,
    command_cycle: bool,
    erstba: u64,
    event_index: u64,
    event_cycle: bool,
    slots: Vec<Slot>,
    /// Each command the controller ran, by its TRB type.
    commands: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct PortRegs {
    powered: bool,
    enabled: bool,
    change: u32,
}

/// The fake controller; cloned, one handle goes to the driver and the test keeps
/// the other.
#[derive(Clone)]
pub struct Fake {
    state: Rc<RefCell<Xhc>>,
    clock: Rc<Cell<u64>>,
}

impl Fake {
    /// A controller whose root ports 1, 2, ... have `ports` plugged in.
    pub fn new(ports: Vec<Option<Device>>) -> Self {
        // As QEMU's are at start: a connected port has its connect change set.
        let root = ports
            .iter()
            .map(|device| PortRegs {
                change: u32::from(device.is_some()) * port::CONNECT_CHANGE,
                ..PortRegs::default()
            })
            .collect();
        Self {
            state: Rc::new(RefCell::new(Xhc {
                ports,
                root,
                pages: BTreeMap::new(),
                next_page: 0x1_0000_0000,
                usbcmd: 0,
                dcbaap: 0,
                crcr: 0,
                command_cycle: true,
                erstba: 0,
                event_index: 0,
                event_cycle: true,
                slots: Vec::new(),
                commands: Vec::new(),
            })),
            clock: Rc::new(Cell::new(0)),
        }
    }

    /// What the driver put in slot `slot`'s contexts.
    pub fn slot(&self, slot: u8) -> SlotRecord {
        self.state.borrow().slots[usize::from(slot) - 1].record
    }

    pub fn slots(&self) -> u8 {
        self.state.borrow().slots.len() as u8
    }

    /// The virtual time, in ns.
    pub fn now(&self) -> u64 {
        self.clock.get()
    }

    /// The commands the controller ran, by TRB type.
    pub fn commands(&self) -> Vec<u8> {
        self.state.borrow().commands.clone()
    }

    /// Whether slot `slot` is enabled (Enable Slot and no Disable Slot since).
    pub fn enabled(&self, slot: u8) -> bool {
        self.state.borrow().slots[usize::from(slot) - 1].enabled
    }

    /// DMA pages the driver was given.
    pub fn pages(&self) -> usize {
        self.state.borrow().pages.len()
    }

    /// Takes the device off root port `port`: it is no longer connected or
    /// enabled, the connect change is set, and a Port Status Change event
    /// written.
    pub fn unplug(&self, port: u8) {
        let mut state = self.state.borrow_mut();
        let number = usize::from(port) - 1;
        state.ports[number] = None;
        state.root[number].enabled = false;
        state.root[number].change |= port::CONNECT_CHANGE;
        state.port_event(port);
    }

    /// Puts `device` on root port `port`, which is then connected but not
    /// enabled until reset, with the connect change set and an event written.
    pub fn plug(&self, port: u8, device: Device) {
        let mut state = self.state.borrow_mut();
        let number = usize::from(port) - 1;
        state.ports[number] = Some(device);
        state.root[number].enabled = false;
        state.root[number].change |= port::CONNECT_CHANGE;
        state.port_event(port);
    }

    /// Completes the transfer queued on `slot`'s interrupt endpoint `dci` with
    /// `bytes` as the report. False when nothing is queued there.
    pub fn report(&self, slot: u8, dci: u8, bytes: &[u8]) -> bool {
        let mut state = self.state.borrow_mut();
        let Some(&(at, cycle)) = state.slots[usize::from(slot) - 1].rings.get(&dci) else {
            return false;
        };
        let entry = state.trb(at);
        if entry.cycle() != cycle || entry.kind() != trb::NORMAL {
            return false;
        }
        let buffer = entry.parameter();
        for (offset, byte) in bytes.iter().enumerate() {
            // SAFETY: the driver's report buffer, at least a report long.
            unsafe { state.page_of(buffer + offset as u64).write_volatile(*byte) };
        }
        let len = entry.0[2] & 0x1_FFFF;
        let residue = len.saturating_sub(bytes.len() as u32);
        state.slots[usize::from(slot) - 1]
            .rings
            .insert(dci, (at + 16, cycle));
        let control = (u32::from(trb::TRANSFER_EVENT) << 10)
            | (u32::from(dci) << 16)
            | (u32::from(slot) << 24);
        let code = if residue > 0 {
            trb::SHORT_PACKET
        } else {
            trb::SUCCESS
        };
        state.event(at, (u32::from(code) << 24) | residue, control);
        true
    }
}

impl Xhc {
    /// The device at `root_port` and `route`, if its root port is enabled.
    fn device_at(&mut self, root_port: u8, route: u32) -> Option<&mut Device> {
        let number = usize::from(root_port).checked_sub(1)?;
        if !self.root.get(number)?.enabled {
            return None;
        }
        self.ports.get_mut(number)?.as_mut()?.at(route)
    }

    fn page_of(&self, phys: u64) -> *mut u8 {
        let base = phys & !0xFFF;
        let page = self
            .pages
            .get(&base)
            .expect("a DMA address nobody was given");
        page.wrapping_add((phys & 0xFFF) as usize)
    }

    fn read32(&self, phys: u64) -> u32 {
        // SAFETY: inside a leaked page, which is 4-aligned and zeroed.
        unsafe { self.page_of(phys).cast::<u32>().read_volatile() }
    }

    fn write32(&self, phys: u64, value: u32) {
        // SAFETY: as in read32.
        unsafe { self.page_of(phys).cast::<u32>().write_volatile(value) }
    }

    fn trb(&self, phys: u64) -> trb::Trb {
        trb::Trb([
            self.read32(phys),
            self.read32(phys + 4),
            self.read32(phys + 8),
            self.read32(phys + 12),
        ])
    }

    /// Writes an event into the event ring.
    fn event(&mut self, parameter: u64, status: u32, control: u32) {
        let segment =
            u64::from(self.read32(self.erstba)) | (u64::from(self.read32(self.erstba + 4)) << 32);
        let size = u64::from(self.read32(self.erstba + 8));
        let at = segment + self.event_index * 16;
        self.write32(at, parameter as u32);
        self.write32(at + 4, (parameter >> 32) as u32);
        self.write32(at + 8, status);
        self.write32(at + 12, control | u32::from(self.event_cycle));
        self.event_index += 1;
        if self.event_index == size {
            self.event_index = 0;
            self.event_cycle = !self.event_cycle;
        }
    }

    fn port_event(&mut self, port: u8) {
        let control = u32::from(trb::PORT_STATUS_CHANGE) << 10;
        self.event(
            u64::from(port) << 24,
            u32::from(trb::SUCCESS) << 24,
            control,
        );
    }

    fn complete(&mut self, command: u64, code: u8, slot: u8) {
        let control = (u32::from(trb::COMMAND_COMPLETION) << 10) | (u32::from(slot) << 24);
        self.event(command, u32::from(code) << 24, control);
    }

    fn run_commands(&mut self) {
        loop {
            let at = self.crcr;
            let command = self.trb(at);
            if command.cycle() != self.command_cycle {
                return;
            }
            if command.kind() == trb::LINK {
                self.crcr = command.parameter();
                self.command_cycle = !self.command_cycle;
                continue;
            }
            self.crcr += 16;
            self.commands.push(command.kind());
            let slot = (command.0[3] >> 24) as u8;
            let (code, slot) = match command.kind() {
                trb::ENABLE_SLOT => {
                    // A disabled slot is given out again first, as hardware may.
                    match self.slots.iter().position(|s| !s.enabled) {
                        Some(at) => {
                            self.slots[at] = Slot::new();
                            (trb::SUCCESS, at as u8 + 1)
                        }
                        None => {
                            self.slots.push(Slot::new());
                            (trb::SUCCESS, self.slots.len() as u8)
                        }
                    }
                }
                trb::DISABLE_SLOT => match self.slots.get_mut(usize::from(slot).wrapping_sub(1)) {
                    Some(entry) if entry.enabled => {
                        entry.enabled = false;
                        entry.rings.clear();
                        (trb::SUCCESS, slot)
                    }
                    // Slot Not Enabled.
                    _ => (11, slot),
                },
                trb::ADDRESS_DEVICE => (self.address(command.parameter(), slot), slot),
                trb::CONFIGURE_ENDPOINT => (self.configure(command.parameter(), slot), slot),
                trb::SET_DEQUEUE => {
                    let pointer = command.parameter();
                    if let Some(entry) = self.slots.get_mut(usize::from(slot) - 1) {
                        entry.ep0 = pointer & !0xF;
                        entry.cycle = pointer & 1 != 0;
                    }
                    (trb::SUCCESS, slot)
                }
                _ => (trb::SUCCESS, slot),
            };
            self.complete(at, code, slot);
        }
    }

    /// Reads the slot context of the input context at `input`.
    fn slot_context(&self, input: u64) -> SlotRecord {
        // 32-byte contexts: the input control context, then the slot's.
        let at = input + 32;
        let (d0, d1, d2) = (self.read32(at), self.read32(at + 4), self.read32(at + 8));
        SlotRecord {
            speed: ((d0 >> 20) & 0xF) as u8,
            route: d0 & 0xF_FFFF,
            root_port: (d1 >> 16) as u8,
            hub: d0 & (1 << 26) != 0,
            hub_ports: (d1 >> 24) as u8,
            think_time: ((d2 >> 16) & 3) as u8,
            parent_slot: d2 as u8,
            parent_port: (d2 >> 8) as u8,
            addressed: false,
            configures: 0,
            raw: [d0, d1, d2, self.read32(at + 12)],
        }
    }

    fn address(&mut self, input: u64, slot: u8) -> u8 {
        let mut record = self.slot_context(input);
        record.addressed = true;
        // The control endpoint's context follows the slot's.
        let ep0 = input + 64;
        let pointer = u64::from(self.read32(ep0 + 8)) | (u64::from(self.read32(ep0 + 12)) << 32);
        let found = self.device_at(record.root_port, record.route).is_some();
        let entry = &mut self.slots[usize::from(slot) - 1];
        entry.record = record;
        entry.ep0 = pointer & !0xF;
        entry.cycle = pointer & 1 != 0;
        match found {
            true => trb::SUCCESS,
            false => trb::TRANSACTION_ERROR,
        }
    }

    fn configure(&mut self, input: u64, slot: u8) -> u8 {
        let add = self.read32(input + 4);
        let record = self.slot_context(input);
        // Each interrupt endpoint added: its context's TR Dequeue Pointer.
        let mut rings = Vec::new();
        for dci in 2..32u8 {
            if add & (1 << dci) != 0 {
                let at = input + 32 * (u64::from(dci) + 1) + 8;
                let pointer = u64::from(self.read32(at)) | (u64::from(self.read32(at + 4)) << 32);
                rings.push((dci, (pointer & !0xF, pointer & 1 != 0)));
            }
        }
        let entry = &mut self.slots[usize::from(slot) - 1];
        entry.rings.extend(rings);
        entry.record.configures += 1;
        if add & 1 != 0 {
            // The hub fields are what a Configure Endpoint may change.
            entry.record.hub = record.hub;
            entry.record.hub_ports = record.hub_ports;
            entry.record.think_time = record.think_time;
        }
        trb::SUCCESS
    }

    /// A transfer event for `trb` on the control endpoint of `slot`.
    fn transfer(&mut self, trb: u64, code: u8, residue: u32, slot: u8) {
        let control = (u32::from(trb::TRANSFER_EVENT) << 10) | (1 << 16) | (u32::from(slot) << 24);
        self.event(trb, (u32::from(code) << 24) | residue, control);
    }

    /// Runs what is queued on `slot`'s control endpoint.
    fn run_control(&mut self, slot: u8) {
        let index = usize::from(slot) - 1;
        let (mut setup, mut data) = ([0u8; 8], None::<(u64, u32, u64)>);
        loop {
            let at = self.slots[index].ep0;
            let entry = self.trb(at);
            if entry.cycle() != self.slots[index].cycle {
                return;
            }
            if entry.kind() == trb::LINK {
                self.slots[index].ep0 = entry.parameter();
                self.slots[index].cycle = !self.slots[index].cycle;
                continue;
            }
            self.slots[index].ep0 += 16;
            match entry.kind() {
                trb::SETUP => {
                    setup[..4].copy_from_slice(&entry.0[0].to_le_bytes());
                    setup[4..].copy_from_slice(&entry.0[1].to_le_bytes());
                    data = None;
                }
                trb::DATA => data = Some((entry.parameter(), entry.0[2] & 0xFFFF, at)),
                trb::STATUS => {
                    let record = self.slots[index].record;
                    let answer = self
                        .device_at(record.root_port, record.route)
                        .and_then(|device| device.control(setup));
                    let Some(bytes) = answer else {
                        self.transfer(at, trb::STALL, 0, slot);
                        return;
                    };
                    if let Some((buffer, len, trb_at)) = data.take() {
                        for (offset, byte) in bytes.iter().enumerate() {
                            // SAFETY: the driver's data page holds a request's data.
                            unsafe { self.page_of(buffer + offset as u64).write_volatile(*byte) };
                        }
                        let moved = bytes.len() as u32;
                        if moved < len {
                            self.transfer(trb_at, trb::SHORT_PACKET, len - moved, slot);
                        }
                    }
                    self.transfer(at, trb::SUCCESS, 0, slot);
                }
                _ => {}
            }
        }
    }
}

impl Hal for Fake {
    fn dma(&mut self, pages: usize, _below_4g: bool) -> Option<Dma> {
        assert_eq!(pages, 1, "the driver asks for one page at a time");
        let page = Box::leak(Box::new(Page([0; 4096])));
        let mut state = self.state.borrow_mut();
        let phys = state.next_page;
        state.next_page += 4096;
        let virt = page.0.as_mut_ptr();
        state.pages.insert(phys, virt);
        // SAFETY: a zeroed page, leaked so it outlives the controller, used by
        // nothing else but the fake, which reads it between the driver's calls.
        Some(unsafe { Dma::new(phys, core::ptr::NonNull::new(virt)?, 4096) })
    }

    fn read32(&self, offset: usize) -> u32 {
        let state = self.state.borrow();
        let ports = state.ports.len();
        match offset {
            regs::CAPLENGTH => (0x0100 << 16) | OPERATIONAL as u32,
            regs::HCSPARAMS1 => u32::from(SLOTS) | ((ports as u32) << 24),
            regs::HCSPARAMS2 => 0,
            // AC64 and port power control; 32-byte contexts, no extended caps.
            regs::HCCPARAMS1 => 1 | (1 << 3),
            regs::DBOFF => DOORBELLS as u32,
            regs::RTSOFF => RUNTIME as u32,
            o if o == OPERATIONAL + regs::USBCMD => state.usbcmd,
            o if o == OPERATIONAL + regs::USBSTS => u32::from(state.usbcmd & regs::CMD_RUN == 0),
            o if o == OPERATIONAL + regs::PAGESIZE => 1,
            o if o >= OPERATIONAL + PORTS => {
                let number = (o - OPERATIONAL - PORTS) / 16;
                match (state.ports.get(number), state.root.get(number)) {
                    (Some(None), Some(regs)) if (o - OPERATIONAL - PORTS).is_multiple_of(16) => {
                        regs.change | (u32::from(regs.powered) * port::POWER)
                    }
                    (Some(Some(device)), Some(regs))
                        if (o - OPERATIONAL - PORTS).is_multiple_of(16) =>
                    {
                        let mut portsc = port::CONNECTED | regs.change;
                        portsc |= u32::from(regs.powered) * port::POWER;
                        portsc |= u32::from(regs.enabled) * (1 << 1);
                        if regs.enabled {
                            portsc |= u32::from(device.speed) << 10;
                        }
                        portsc
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    fn write32(&self, offset: usize, value: u32) {
        let mut state = self.state.borrow_mut();
        match offset {
            o if o == OPERATIONAL + regs::USBCMD => {
                state.usbcmd = value & !regs::CMD_RESET;
                if value & regs::CMD_RESET != 0 {
                    state.usbcmd = 0;
                }
            }
            o if o == OPERATIONAL + regs::CRCR => {
                state.crcr = (state.crcr & !0xFFFF_FFFF) | u64::from(value & !0x3F);
                state.command_cycle = value & 1 != 0;
            }
            o if o == OPERATIONAL + regs::CRCR + 4 => {
                state.crcr = (state.crcr & 0xFFFF_FFFF) | (u64::from(value) << 32);
            }
            o if o == OPERATIONAL + regs::DCBAAP => {
                state.dcbaap = (state.dcbaap & !0xFFFF_FFFF) | u64::from(value);
            }
            o if o == OPERATIONAL + regs::DCBAAP + 4 => {
                state.dcbaap = (state.dcbaap & 0xFFFF_FFFF) | (u64::from(value) << 32);
            }
            o if o == RUNTIME + regs::ERSTBA => {
                state.erstba = (state.erstba & !0xFFFF_FFFF) | u64::from(value);
            }
            o if o == RUNTIME + regs::ERSTBA + 4 => {
                state.erstba = (state.erstba & 0xFFFF_FFFF) | (u64::from(value) << 32);
            }
            o if (OPERATIONAL + PORTS..DOORBELLS).contains(&o) => {
                let number = (o - OPERATIONAL - PORTS) / 16;
                if !(o - OPERATIONAL - PORTS).is_multiple_of(16) || number >= state.root.len() {
                    return;
                }
                let attached = state.ports[number].is_some();
                let regs = &mut state.root[number];
                regs.change &= !(value & port::CHANGES);
                if value & port::POWER != 0 {
                    regs.powered = true;
                }
                if value & port::RESET != 0 && attached {
                    regs.enabled = true;
                    regs.change |= port::RESET_CHANGE;
                    state.port_event(number as u8 + 1);
                }
            }
            o if (DOORBELLS..DOORBELLS + 4 * 256).contains(&o) => {
                let slot = ((o - DOORBELLS) / 4) as u8;
                match slot {
                    0 => state.run_commands(),
                    slot if value == 1 => state.run_control(slot),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn now_ns(&self) -> u64 {
        // Every read of the clock is a microsecond on.
        self.clock.set(self.clock.get() + 1000);
        self.clock.get()
    }

    fn delay(&self, ns: u64) {
        self.clock.set(self.clock.get() + ns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_reached_through_a_port_that_is_not_powered_and_enabled() {
        let mut hub = Device::hub(vec![Some(Device::keyboard())]);
        assert!(hub.at(1).is_none(), "an unpowered port");
        let Kind::Hub(ports) = &mut hub.kind else {
            unreachable!()
        };
        let behind = ports[0].as_mut().unwrap();
        behind.powered = true;
        assert!(hub.at(1).is_none(), "a port not reset yet");
        let Kind::Hub(ports) = &mut hub.kind else {
            unreachable!()
        };
        ports[0].as_mut().unwrap().enabled = true;
        assert!(hub.at(1).is_some());
        assert!(hub.at(2).is_none(), "no such port");
    }
}
