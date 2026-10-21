//! Setting up the devices on the root ports: reset the port, enable a slot,
//! address the device, read its descriptors, pick its HID interfaces, configure
//! it and its interrupt endpoints, then queue the first report transfer on each.

use super::{Controller, Decode, Device, DeviceKind, Endpoint, RESET_TIMEOUT_NS, Recovery};
use crate::context::{self, Endpoint as EndpointContext};
use crate::descriptor::{self, Configuration, HidInterface, MAX_HID};
use crate::hal::{Dma, Hal};
use crate::regs::{port, speed};
use crate::ring::{self, Producer};
use crate::trb::Trb;
use crate::{Error, PAGE, hid};

/// Settle time after power-up or a connect (USB 2.0 TATTDB, 100 ms) and after
/// a reset (TRSTRCY, 10 ms).
const SETTLE_NS: u64 = 100_000_000;
const RESET_RECOVERY_NS: u64 = 10_000_000;
/// Report buffers are this long at most; HID reports are shorter.
const REPORT_BYTES: u16 = 64;
/// The longest descriptor read: HID devices' configurations and report
/// descriptors are far shorter, and the copy lives on the caller's stack.
const READ_BYTES: usize = 1024;

/// A device that was set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attached {
    pub speed: u8,
    pub vendor: u16,
    pub product: u16,
    /// What each kept HID interface is.
    pub kinds: [Option<DeviceKind>; MAX_HID],
}

/// What happened on one connected root port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortReport {
    pub port: u8,
    pub outcome: Result<Attached, Error>,
}

impl<H: Hal> Controller<H> {
    /// Sets up the device on each connected root port and starts listening to
    /// its HID interfaces. `report` hears about each connected port.
    pub fn enumerate(&mut self, mut report: impl FnMut(PortReport)) {
        if self.caps.port_power {
            for number in 1..=self.caps.max_ports {
                let at = self.caps.portsc(number);
                let portsc = self.hal.read32(at);
                if portsc & port::POWER == 0 {
                    self.hal.write32(at, port::write_value(portsc, port::POWER));
                }
            }
        }
        self.hal.delay(SETTLE_NS);
        for number in 1..=self.caps.max_ports {
            if self.hal.read32(self.caps.portsc(number)) & port::CONNECTED == 0 {
                continue;
            }
            let outcome = self.attach(number);
            report(PortReport {
                port: number,
                outcome,
            });
        }
        for index in 0..self.endpoints.len() {
            self.arm(index);
        }
    }

    /// Resets `number` if it is not enabled yet (USB 2 ports; USB 3 ports enable
    /// themselves) and returns its speed.
    fn reset_port(&self, number: u8) -> Result<u8, Error> {
        let at = self.caps.portsc(number);
        let portsc = self.hal.read32(at);
        if portsc & port::ENABLED == 0 {
            self.hal.write32(at, port::write_value(portsc, port::RESET));
            self.wait(RESET_TIMEOUT_NS, |c| {
                c.hal.read32(at) & port::RESET_CHANGE != 0
            })?;
            let portsc = self.hal.read32(at);
            self.hal
                .write32(at, port::write_value(portsc, port::RESET_CHANGE));
            self.hal.delay(RESET_RECOVERY_NS);
        }
        let portsc = self.hal.read32(at);
        if portsc & port::ENABLED == 0 {
            return Err(Error::Timeout);
        }
        Ok(port::speed(portsc))
    }

    fn page(&mut self) -> Result<Dma, Error> {
        self.hal.dma(1, !self.caps.addr64).ok_or(Error::NoMemory)
    }

    fn attach(&mut self, number: u8) -> Result<Attached, Error> {
        let speed = self.reset_port(number)?;
        let index = self
            .devices
            .iter()
            .position(Option::is_none)
            .ok_or(Error::TooMany)?;
        let slot = self.command(Trb::enable_slot())?;
        if usize::from(slot) > self.caps.max_slots.into() || slot == 0 {
            return Err(Error::Completion(0));
        }
        let output = self.page()?;
        let ep0_mem = self.page()?;
        self.dcbaa.write64(usize::from(slot) * 8, output.phys());
        self.devices[index] = Some(Device {
            slot,
            ep0: Producer::new(ep0_mem.phys(), ring::ENTRIES),
            ep0_mem,
            _output: output,
            input: self.page()?,
            data: self.page()?,
            reports: self.page()?,
        });
        let packet = speed::default_packet_size(speed);
        self.write_input(
            index,
            0b11,
            1,
            &[(1, EndpointContext::control(packet, 0))],
            speed,
            number,
        );
        let input = self.input_phys(index);
        self.command(Trb::address_device(input, slot))?;
        let first = self.read(index, descriptor::get_descriptor(descriptor::DEVICE, 0, 8))?;
        let packet0 = descriptor::Device::max_packet0(&first)?;
        if packet0 != packet && packet0 != 0 {
            self.write_input(
                index,
                0b10,
                1,
                &[(1, EndpointContext::control(packet0, 0))],
                speed,
                number,
            );
            self.command(Trb::evaluate_context(input, slot))?;
        }
        let device_bytes = descriptor::DEVICE_BYTES;
        let device = descriptor::Device::parse(&self.read(
            index,
            descriptor::get_descriptor(descriptor::DEVICE, 0, device_bytes),
        )?)?;
        let head = self.read(
            index,
            descriptor::get_descriptor(
                descriptor::CONFIGURATION,
                0,
                descriptor::CONFIGURATION_BYTES,
            ),
        )?;
        let total = descriptor::configuration_length(&head)?.min(READ_BYTES as u16);
        let config = Configuration::parse(&self.read(
            index,
            descriptor::get_descriptor(descriptor::CONFIGURATION, 0, total),
        )?)?;
        let mut chosen = [None; MAX_HID];
        for (at, interface) in config.interfaces().enumerate() {
            chosen[at] = self
                .decode_for(index, interface)
                .map(|decode| (*interface, decode));
        }
        self.control(index, descriptor::set_configuration(config.value))?;
        let mut kinds = [None; MAX_HID];
        for (at, (interface, decode)) in chosen.iter().flatten().enumerate() {
            // Both are optional for a device: a stall or another error code
            // leaves it as it was, but a device that stops answering is dropped.
            if matches!(decode, Decode::BootKeyboard(_) | Decode::BootMouse) {
                optional(self.control(index, descriptor::set_protocol(interface.number, true)))?;
            }
            if matches!(decode, Decode::BootKeyboard(_)) {
                optional(self.control(index, descriptor::set_idle(interface.number)))?;
            }
            kinds[at] = Some(kind_of(decode));
        }
        self.configure_endpoints(index, speed, number, &chosen)?;
        Ok(Attached {
            speed,
            vendor: device.vendor,
            product: device.product,
            kinds,
        })
    }

    /// Runs a reading control transfer and returns what came, up to
    /// [`READ_BYTES`].
    fn read(&mut self, index: usize, request: descriptor::Request) -> Result<ReadBack, Error> {
        let len = self.control(index, request)?.min(READ_BYTES);
        let mut back = ReadBack {
            bytes: [0; READ_BYTES],
            len,
        };
        if let Some(device) = self.devices[index].as_ref() {
            device.data.read_bytes(0, &mut back.bytes[..len]);
        }
        Ok(back)
    }

    /// How to read `interface`'s reports, or None to leave it alone.
    fn decode_for(&mut self, index: usize, interface: &HidInterface) -> Option<Decode> {
        if interface.boot_keyboard() {
            return Some(Decode::BootKeyboard(hid::Keyboard::new()));
        }
        // A tablet may call itself a boot mouse, so its report descriptor is read
        // first: absolute X and Y make it a tablet whatever its protocol says.
        let layout = match interface.report_length {
            0 => None,
            length => {
                let length = length.min(READ_BYTES as u16);
                let request = descriptor::get_report_descriptor(interface.number, length);
                self.read(index, request)
                    .ok()
                    .and_then(|bytes| hid::pointer_layout(&bytes))
            }
        };
        match layout {
            Some(layout) if layout.absolute() => Some(Decode::Report(DeviceKind::Tablet, layout)),
            _ if interface.boot_mouse() => Some(Decode::BootMouse),
            Some(layout) => Some(Decode::Report(DeviceKind::Mouse, layout)),
            None => None,
        }
    }

    fn input_phys(&self, index: usize) -> u64 {
        self.devices[index].as_ref().map_or(0, |d| d.input.phys())
    }

    /// Fills device `index`'s input context: the add flags, the slot context
    /// (its last context `last`) and each (dci, endpoint). A control endpoint
    /// given ring 0 gets the device's EP0 ring.
    fn write_input(
        &mut self,
        index: usize,
        add: u32,
        last: u8,
        endpoints: &[(u8, EndpointContext)],
        speed: u8,
        port: u8,
    ) {
        let bytes = self.caps.context_bytes;
        let Some(device) = self.devices[index].as_mut() else {
            return;
        };
        device.input.zero(0, PAGE);
        device.input.write_dwords(0, &context::input_control(add));
        device.input.write_dwords(
            context::input_offset(0, bytes),
            &context::slot(speed, port, last),
        );
        for (dci, endpoint) in endpoints {
            let mut endpoint = *endpoint;
            if endpoint.kind == context::CONTROL && endpoint.ring == 0 {
                endpoint.ring = device.ep0.base();
            }
            device
                .input
                .write_dwords(context::input_offset(*dci, bytes), &endpoint.dwords());
        }
    }

    /// Gives each chosen interface's interrupt-IN endpoint a ring and a report
    /// buffer, and tells the controller with Configure Endpoint.
    fn configure_endpoints(
        &mut self,
        index: usize,
        speed: u8,
        port: u8,
        chosen: &[Option<(HidInterface, Decode)>; MAX_HID],
    ) -> Result<(), Error> {
        let slot = self.devices[index].as_ref().ok_or(Error::TooMany)?.slot;
        let wanted = chosen.iter().flatten().count();
        if self.endpoints.iter().filter(|e| e.is_none()).count() < wanted {
            return Err(Error::TooMany);
        }
        let mut contexts = [(0, EndpointContext::control(0, 0)); MAX_HID];
        let mut pending = [const { None }; MAX_HID];
        let (mut add, mut last) = (1, 1);
        for (at, (interface, decode)) in chosen.iter().flatten().enumerate() {
            let ring_mem = self.page()?;
            let dci = context::dci(interface.endpoint);
            let interval = context::interrupt_interval(speed, interface.interval);
            let packet = interface.max_packet.max(1);
            contexts[at] = (
                dci,
                EndpointContext::interrupt_in(packet, interval, ring_mem.phys()),
            );
            add |= 1 << dci;
            last = last.max(dci);
            pending[at] = Some(Endpoint {
                slot,
                dci,
                ring: Producer::new(ring_mem.phys(), ring::ENTRIES),
                ring_mem,
                device: index,
                buffer: at * usize::from(REPORT_BYTES),
                len: packet.min(REPORT_BYTES),
                decode: *decode,
                armed: false,
                recovery: Recovery::Idle,
                failures: 0,
            });
        }
        if wanted == 0 {
            return Ok(());
        }
        self.write_input(index, add, last, &contexts[..wanted], speed, port);
        let input = self.input_phys(index);
        self.command(Trb::configure_endpoint(input, slot))?;
        // Only endpoints the controller now knows are kept, and so armed.
        for endpoint in pending.into_iter().flatten() {
            if let Some(free) = self.endpoints.iter_mut().find(|e| e.is_none()) {
                *free = Some(endpoint);
            }
        }
        Ok(())
    }
}

/// Lets a request the device may refuse fail, but not time out.
fn optional(result: Result<usize, Error>) -> Result<(), Error> {
    match result {
        Err(Error::Timeout) => Err(Error::Timeout),
        _ => Ok(()),
    }
}

fn kind_of(decode: &Decode) -> DeviceKind {
    match decode {
        Decode::BootKeyboard(_) => DeviceKind::Keyboard,
        Decode::BootMouse => DeviceKind::Mouse,
        Decode::Report(kind, _) => *kind,
    }
}

/// Bytes a control read brought back.
struct ReadBack {
    bytes: [u8; READ_BYTES],
    len: usize,
}

impl core::ops::Deref for ReadBack {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
