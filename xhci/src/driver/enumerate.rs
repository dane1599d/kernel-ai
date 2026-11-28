//! Setting up the devices on the root ports: reset the port, enable a slot,
//! address the device, read its descriptors, pick its HID interfaces, configure
//! it and its interrupt endpoints, then queue the first report transfer on each.
//! A hub (USB 2.0 chapter 11) is set up the same way, then each of its ports is
//! powered and reset with hub class requests, and what is behind it is set up in
//! turn, down to [`MAX_TIERS`] hubs deep.

use super::{Controller, Decode, Device, DeviceKind, Endpoint, RESET_TIMEOUT_NS, Recovery};
use crate::context::{self, Endpoint as EndpointContext, Hub, Path};
use crate::descriptor::{self, Configuration, HidInterface, MAX_HID, PortStatus};
use crate::hal::{Dma, Hal};
use crate::regs::{port, speed};
use crate::ring::{self, Producer};
use crate::trb::Trb;
use crate::{Error, PAGE, hid};

/// Settle time after power-up or a connect (USB 2.0 TATTDB, 100 ms) and after
/// a reset (TRSTRCY, 10 ms).
pub(super) const SETTLE_NS: u64 = 100_000_000;
const RESET_RECOVERY_NS: u64 = 10_000_000;
/// Hubs deep a device may be: the route string has five nibbles (xHCI 8.9), so
/// five hubs, and a device behind the fifth.
pub const MAX_TIERS: u8 = 5;
/// How long a hub port may take to finish its reset (USB 2.0 TDRST is 50 ms at
/// most), and how long all the hub ports of one `enumerate` may spend waiting:
/// a broken hub must not hold the boot.
const HUB_RESET_TIMEOUT_NS: u64 = 250_000_000;
pub(super) const HUB_BUDGET_NS: u64 = 2_000_000_000;
/// How often a hub port's status is read while it resets.
const POLL_NS: u64 = 10_000_000;
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
    /// For a hub, its number of ports.
    pub hub_ports: Option<u8>,
}

/// What happened on one connected root port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortReport {
    /// The root port: for a device behind hubs, the one its hubs hang from.
    pub port: u8,
    /// The route string: the port on each hub on the way, the first hub's in the
    /// lowest nibble. 0 for a device on the root port itself.
    pub route: u32,
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
        self.hub_deadline = self.hal.now_ns().saturating_add(HUB_BUDGET_NS);
        for number in 1..=self.caps.max_ports {
            let at = self.caps.portsc(number);
            let portsc = self.hal.read32(at);
            if portsc & port::CONNECTED == 0 {
                continue;
            }
            // The connect is handled here: `service_ports` acts on a connect
            // change only, so the port's events from this setup are not taken
            // for a new device.
            self.hal
                .write32(at, port::write_value(portsc, portsc & port::CONNECT_CHANGE));
            let path = Path {
                root_port: number,
                ..Path::default()
            };
            let outcome = match self.reset_port(number) {
                Ok(speed) => self.attach(path, speed, 0, &mut report),
                Err(error) => Err(error),
            };
            report(PortReport {
                port: number,
                route: 0,
                outcome,
            });
        }
        for index in 0..self.endpoints.len() {
            self.arm(index);
        }
    }

    /// Resets `number` if it is not enabled yet (USB 2 ports; USB 3 ports enable
    /// themselves) and returns its speed.
    pub(super) fn reset_port(&self, number: u8) -> Result<u8, Error> {
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

    /// A zeroed page: a detached device's, or a new one.
    fn page(&mut self) -> Result<Dma, Error> {
        if let Some(mut page) = self.spare.iter_mut().find_map(Option::take) {
            let len = page.len();
            page.zero(0, len);
            return Ok(page);
        }
        self.hal.dma(1, !self.caps.addr64).ok_or(Error::NoMemory)
    }

    /// Sets up the device at `path`, whose port is reset and at `speed`; `tier`
    /// is the number of hubs above it. What is behind it, if it is a hub, is
    /// reported through `report` before this returns.
    pub(super) fn attach(
        &mut self,
        path: Path,
        speed: u8,
        tier: u8,
        report: &mut dyn FnMut(PortReport),
    ) -> Result<Attached, Error> {
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
            root_port: path.root_port,
            route: path.route,
            ep0: Producer::new(ep0_mem.phys(), ring::ENTRIES),
            ep0_mem,
            output,
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
            &path,
            None,
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
                &path,
                None,
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
        if device.class == descriptor::CLASS_HUB {
            return self.attach_hub(index, speed, path, tier, &device, config.value, report);
        }
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
        self.configure_endpoints(index, speed, &path, &chosen)?;
        Ok(Attached {
            speed,
            vendor: device.vendor,
            product: device.product,
            kinds,
            hub_ports: None,
        })
    }

    /// Sets up a hub and what is behind it. The hub is configured, told it is a
    /// hub in its slot context, and each of its ports is powered; a port with
    /// something connected is reset, and the device on it is set up like any
    /// other, below this hub.
    #[allow(clippy::too_many_arguments)]
    fn attach_hub(
        &mut self,
        index: usize,
        speed: u8,
        path: Path,
        tier: u8,
        device: &descriptor::Device,
        configuration: u8,
        report: &mut dyn FnMut(PortReport),
    ) -> Result<Attached, Error> {
        if device.usb >= 0x0300 || speed >= speed::SUPER || tier >= MAX_TIERS {
            // A SuperSpeed hub has its own descriptor and depth, and its ports
            // reset differently; a sixth hub has no nibble left in the route.
            return Err(match tier >= MAX_TIERS {
                true => Error::TooMany,
                false => Error::Unsupported,
            });
        }
        let slot = self.devices[index].as_ref().ok_or(Error::TooMany)?.slot;
        self.control(index, descriptor::set_configuration(configuration))?;
        let bytes = self.read(
            index,
            descriptor::get_hub_descriptor(descriptor::HUB_DESCRIPTOR_MAX),
        )?;
        let hub = descriptor::HubDescriptor::parse(&bytes)?;
        let info = Hub {
            ports: hub.ports,
            think_time: hub.think_time,
        };
        self.write_input(index, 1, 1, &[], speed, &path, Some(info));
        let input = self.input_phys(index);
        self.command(Trb::configure_endpoint(input, slot))?;
        let attached = Attached {
            speed,
            vendor: device.vendor,
            product: device.product,
            kinds: [None; MAX_HID],
            hub_ports: Some(hub.ports),
        };
        let ports = hub.ports.min(descriptor::MAX_HUB_PORTS);
        for port in 1..=ports {
            self.control(
                index,
                descriptor::set_port_feature(port, descriptor::PORT_POWER),
            )?;
        }
        self.hal.delay(u64::from(hub.power_good_ms) * 1_000_000);
        self.hal.delay(SETTLE_NS);
        for port in 1..=ports {
            let status = self.port_status(index, port)?;
            if !status.connected() {
                continue;
            }
            let route = path.route | (u32::from(port) << (4 * u32::from(tier)));
            let outcome = match self.reset_hub_port(index, port) {
                Ok(status) => {
                    let child_speed = status.speed();
                    // A low- or full-speed device behind a high-speed hub talks
                    // through that hub's translator; behind a full-speed hub, the
                    // translator its hub already uses.
                    let translator = match child_speed {
                        speed::LOW | speed::FULL if speed == speed::HIGH => Some((slot, port)),
                        speed::LOW | speed::FULL => path.translator,
                        _ => None,
                    };
                    let child = Path {
                        root_port: path.root_port,
                        route,
                        translator,
                    };
                    self.attach(child, child_speed, tier + 1, report)
                }
                Err(error) => Err(error),
            };
            report(PortReport {
                port: path.root_port,
                route,
                outcome,
            });
        }
        Ok(attached)
    }

    fn port_status(&mut self, index: usize, port: u8) -> Result<PortStatus, Error> {
        let bytes = self.read(index, descriptor::get_port_status(port))?;
        PortStatus::parse(&bytes)
    }

    /// Resets hub port `port` and returns its status once the reset is done and
    /// the port enabled.
    fn reset_hub_port(&mut self, index: usize, port: u8) -> Result<PortStatus, Error> {
        self.control(
            index,
            descriptor::set_port_feature(port, descriptor::PORT_RESET),
        )?;
        let start = self.hal.now_ns();
        if start > self.hub_deadline {
            return Err(Error::Timeout);
        }
        loop {
            self.hal.delay(POLL_NS);
            if self.port_status(index, port)?.reset_done() {
                break;
            }
            let now = self.hal.now_ns();
            if now.wrapping_sub(start) > HUB_RESET_TIMEOUT_NS || now > self.hub_deadline {
                return Err(Error::Timeout);
            }
        }
        for feature in [descriptor::C_PORT_RESET, descriptor::C_PORT_CONNECTION] {
            self.control(index, descriptor::clear_port_feature(port, feature))?;
        }
        self.hal.delay(RESET_RECOVERY_NS);
        let status = self.port_status(index, port)?;
        match status.connected() && status.enabled() {
            true => Ok(status),
            false => Err(Error::Timeout),
        }
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
        // A mouse whose descriptor places a wheel is read by it, in the report
        // protocol it starts in: the boot protocol defines no wheel byte.
        match layout {
            Some(layout) if layout.absolute() => Some(Decode::Report(DeviceKind::Tablet, layout)),
            Some(layout) if layout.wheel.is_some() => {
                Some(Decode::Report(DeviceKind::Mouse, layout))
            }
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
    #[allow(clippy::too_many_arguments)]
    fn write_input(
        &mut self,
        index: usize,
        add: u32,
        last: u8,
        endpoints: &[(u8, EndpointContext)],
        speed: u8,
        path: &Path,
        hub: Option<Hub>,
    ) {
        let bytes = self.caps.context_bytes;
        let Some(device) = self.devices[index].as_mut() else {
            return;
        };
        device.input.zero(0, PAGE);
        device.input.write_dwords(0, &context::input_control(add));
        device.input.write_dwords(
            context::input_offset(0, bytes),
            &context::slot_at(speed, path, last, hub),
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
        path: &Path,
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
        self.write_input(index, add, last, &contexts[..wanted], speed, path, None);
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
