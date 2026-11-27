//! USB standard and HID class requests, and the descriptors the driver reads:
//! the device descriptor and the configuration with its interfaces, HID
//! descriptors and endpoints.

use crate::Error;

/// An 8-byte SETUP packet.
pub type Request = [u8; 8];

const CLEAR_FEATURE: u8 = 1;
const GET_DESCRIPTOR: u8 = 6;
const SET_CONFIGURATION: u8 = 9;
const SET_IDLE: u8 = 0x0A;
const SET_PROTOCOL: u8 = 0x0B;
/// The feature selector CLEAR_FEATURE takes for an endpoint's halt.
const ENDPOINT_HALT: u16 = 0;

pub const DEVICE: u8 = 1;
pub const CONFIGURATION: u8 = 2;
pub const INTERFACE: u8 = 4;
pub const ENDPOINT: u8 = 5;
pub const HID: u8 = 0x21;
pub const REPORT: u8 = 0x22;

pub const DEVICE_BYTES: u16 = 18;
pub const CONFIGURATION_BYTES: u16 = 9;

pub const CLASS_HID: u8 = 3;
pub const CLASS_HUB: u8 = 9;
/// The hub descriptor (USB 2.0 11.23.2.1). A SuperSpeed hub's is 0x2A, which the
/// driver does not read: it leaves such hubs alone.
pub const HUB: u8 = 0x29;

/// Hub class features (USB 2.0 table 11-17) and the change bits that go with them.
pub const PORT_RESET: u16 = 4;
pub const PORT_POWER: u16 = 8;
pub const C_PORT_CONNECTION: u16 = 16;
pub const C_PORT_RESET: u16 = 20;

fn request(kind: u8, request: u8, value: u16, index: u16, length: u16) -> Request {
    let [v0, v1] = value.to_le_bytes();
    let [i0, i1] = index.to_le_bytes();
    let [l0, l1] = length.to_le_bytes();
    [kind, request, v0, v1, i0, i1, l0, l1]
}

/// GET_DESCRIPTOR from the device: `kind` and `index`, `length` bytes.
pub fn get_descriptor(kind: u8, index: u8, length: u16) -> Request {
    request(
        0x80,
        GET_DESCRIPTOR,
        u16::from_be_bytes([kind, index]),
        0,
        length,
    )
}

/// GET_DESCRIPTOR(Report) from HID interface `interface`.
pub fn get_report_descriptor(interface: u8, length: u16) -> Request {
    request(
        0x81,
        GET_DESCRIPTOR,
        u16::from(REPORT) << 8,
        u16::from(interface),
        length,
    )
}

pub fn set_configuration(value: u8) -> Request {
    request(0x00, SET_CONFIGURATION, u16::from(value), 0, 0)
}

/// SET_PROTOCOL on a HID interface: boot (0) or report (1).
pub fn set_protocol(interface: u8, boot: bool) -> Request {
    request(
        0x21,
        SET_PROTOCOL,
        u16::from(!boot),
        u16::from(interface),
        0,
    )
}

/// SET_IDLE with duration 0: report only on change, for every report.
pub fn set_idle(interface: u8) -> Request {
    request(0x21, SET_IDLE, 0, u16::from(interface), 0)
}

/// CLEAR_FEATURE(ENDPOINT_HALT) on endpoint `address`: ends a stall on the
/// device and resets the endpoint's data toggle to DATA0 (USB 2.0 9.4.1, 9.4.5).
pub fn clear_endpoint_halt(address: u8) -> Request {
    request(0x02, CLEAR_FEATURE, ENDPOINT_HALT, u16::from(address), 0)
}

/// GET_DESCRIPTOR(Hub) from a hub, `length` bytes.
pub fn get_hub_descriptor(length: u16) -> Request {
    request(0xA0, GET_DESCRIPTOR, u16::from(HUB) << 8, 0, length)
}

/// GET_STATUS of hub port `port` (1-based): four bytes, status then change.
pub fn get_port_status(port: u8) -> Request {
    request(0xA3, 0, 0, u16::from(port), 4)
}

/// SET_FEATURE on hub port `port`.
pub fn set_port_feature(port: u8, feature: u16) -> Request {
    request(0x23, 3, feature, u16::from(port), 0)
}

/// CLEAR_FEATURE on hub port `port`.
pub fn clear_port_feature(port: u8, feature: u16) -> Request {
    request(0x23, CLEAR_FEATURE, feature, u16::from(port), 0)
}

/// Whether a request moves data from the device.
pub fn reads(request: &Request) -> bool {
    request[0] & 0x80 != 0
}

/// The request's wLength.
pub fn length(request: &Request) -> u16 {
    u16::from_le_bytes([request[6], request[7]])
}

/// What the driver takes from the device descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub usb: u16,
    pub class: u8,
    /// The control endpoint's packet size: a byte count before USB 3, a power of
    /// two after.
    pub max_packet0: u16,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}

impl Device {
    /// Reads the packet size alone from the descriptor's first eight bytes.
    pub fn max_packet0(bytes: &[u8]) -> Result<u16, Error> {
        if bytes.len() < 8 || bytes[1] != DEVICE {
            return Err(Error::BadDescriptor);
        }
        let usb = u16::from_le_bytes([bytes[2], bytes[3]]);
        Ok(match usb >= 0x0300 {
            true => 1 << bytes[7].min(15),
            false => u16::from(bytes[7]),
        })
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < usize::from(DEVICE_BYTES) {
            return Err(Error::BadDescriptor);
        }
        Ok(Self {
            usb: u16::from_le_bytes([bytes[2], bytes[3]]),
            class: bytes[4],
            max_packet0: Self::max_packet0(bytes)?,
            vendor: u16::from_le_bytes([bytes[8], bytes[9]]),
            product: u16::from_le_bytes([bytes[10], bytes[11]]),
            configurations: bytes[17],
        })
    }
}

/// What the driver takes from a hub descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubDescriptor {
    pub ports: u8,
    /// Time from a port's power-on to its power being good, in ms.
    pub power_good_ms: u16,
    /// The think time of the hub's transaction translator, 0 to 3 for 8, 16, 24
    /// and 32 full-speed bit times.
    pub think_time: u8,
}

/// The most a hub descriptor takes: 7 bytes and two bit masks of 255 ports.
/// Asked for whole, since a hub of 8 ports or more is longer than 9 bytes.
pub const HUB_DESCRIPTOR_MAX: u16 = 71;

/// Ports a hub may have (USB 2.0 11.23.2.1: 255), and the most the driver walks.
pub const MAX_HUB_PORTS: u8 = 15;

impl HubDescriptor {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // bLength, type, ports, wHubCharacteristics (2), power-on to good,
        // control current, then the removable and power masks.
        // A hub of 8 ports or more sends more than the 7 bytes read here; what
        // follows (the removable and power masks) is not used.
        if bytes.len() < 7 || bytes[1] != HUB || bytes[0] < 7 {
            return Err(Error::BadDescriptor);
        }
        if bytes[2] == 0 {
            return Err(Error::BadDescriptor);
        }
        let characteristics = u16::from_le_bytes([bytes[3], bytes[4]]);
        Ok(Self {
            ports: bytes[2],
            power_good_ms: u16::from(bytes[5]) * 2,
            think_time: ((characteristics >> 5) & 0x3) as u8,
        })
    }
}

/// A hub port's status (USB 2.0 table 11-21): wPortStatus and wPortChange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortStatus {
    pub status: u16,
    pub change: u16,
}

impl PortStatus {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        match bytes {
            [s0, s1, c0, c1, ..] => Ok(Self {
                status: u16::from_le_bytes([*s0, *s1]),
                change: u16::from_le_bytes([*c0, *c1]),
            }),
            _ => Err(Error::BadDescriptor),
        }
    }

    pub fn connected(&self) -> bool {
        self.status & 1 != 0
    }

    pub fn enabled(&self) -> bool {
        self.status & (1 << 1) != 0
    }

    /// Whether the reset finished (C_PORT_RESET).
    pub fn reset_done(&self) -> bool {
        self.change & (1 << 4) != 0
    }

    /// The device's speed as the xHCI numbers it: low, high or else full.
    pub fn speed(&self) -> u8 {
        use crate::regs::speed;
        match (self.status & (1 << 9) != 0, self.status & (1 << 10) != 0) {
            (true, _) => speed::LOW,
            (_, true) => speed::HIGH,
            _ => speed::FULL,
        }
    }
}

/// The configuration descriptor's total length, from its first nine bytes.
pub fn configuration_length(bytes: &[u8]) -> Result<u16, Error> {
    if bytes.len() < usize::from(CONFIGURATION_BYTES) || bytes[1] != CONFIGURATION {
        return Err(Error::BadDescriptor);
    }
    Ok(u16::from_le_bytes([bytes[2], bytes[3]]))
}

/// A HID interface with an interrupt-IN endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidInterface {
    pub number: u8,
    pub subclass: u8,
    pub protocol: u8,
    /// The report descriptor's length from the HID descriptor.
    pub report_length: u16,
    pub endpoint: u8,
    pub max_packet: u16,
    pub interval: u8,
}

impl HidInterface {
    /// Boot interface subclass, keyboard protocol.
    pub fn boot_keyboard(&self) -> bool {
        self.subclass == 1 && self.protocol == 1
    }

    pub fn boot_mouse(&self) -> bool {
        self.subclass == 1 && self.protocol == 2
    }
}

/// The parts of a configuration the driver uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub hid: [Option<HidInterface>; MAX_HID],
}

/// HID interfaces kept per device; a keyboard with a media-key interface has two.
pub const MAX_HID: usize = 4;

impl Configuration {
    /// Walks the whole configuration descriptor. Interfaces other than HID, and
    /// alternate settings other than 0, are skipped.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let total = usize::from(configuration_length(bytes)?).min(bytes.len());
        let mut config = Self {
            value: bytes[5],
            hid: [None; MAX_HID],
        };
        let mut current: Option<HidInterface> = None;
        let mut kept = 0;
        let mut at = 0;
        while at + 2 <= total {
            let len = usize::from(bytes[at]);
            if len < 2 || at + len > total {
                return Err(Error::BadDescriptor);
            }
            let body = &bytes[at..at + len];
            match body[1] {
                INTERFACE if len >= 9 => {
                    current = (body[5] == CLASS_HID && body[3] == 0).then_some(HidInterface {
                        number: body[2],
                        subclass: body[6],
                        protocol: body[7],
                        report_length: 0,
                        endpoint: 0,
                        max_packet: 0,
                        interval: 0,
                    });
                }
                HID if len >= 9 => {
                    if let Some(interface) = current.as_mut() {
                        // The class descriptors follow as (type, length) pairs.
                        for pair in body[6..].as_chunks::<3>().0 {
                            if pair[0] == REPORT {
                                interface.report_length = u16::from_le_bytes([pair[1], pair[2]]);
                            }
                        }
                    }
                }
                ENDPOINT if len >= 7 => {
                    let interrupt_in = body[3] & 0x3 == 3 && body[2] & 0x80 != 0;
                    if let Some(mut interface) = current.filter(|_| interrupt_in) {
                        interface.endpoint = body[2];
                        interface.max_packet = u16::from_le_bytes([body[4], body[5]]) & 0x7FF;
                        interface.interval = body[6];
                        if kept == MAX_HID {
                            return Err(Error::TooMany);
                        }
                        config.hid[kept] = Some(interface);
                        kept += 1;
                        current = None;
                    }
                }
                _ => {}
            }
            at += len;
        }
        Ok(config)
    }

    pub fn interfaces(&self) -> impl Iterator<Item = &HidInterface> {
        self.hid.iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_encode_little_endian() {
        assert_eq!(get_descriptor(DEVICE, 0, 18), [0x80, 6, 0, 1, 0, 0, 18, 0]);
        assert_eq!(
            get_descriptor(CONFIGURATION, 0, 0x22),
            [0x80, 6, 0, 2, 0, 0, 0x22, 0]
        );
        assert_eq!(
            get_report_descriptor(1, 74),
            [0x81, 6, 0, 0x22, 1, 0, 74, 0]
        );
        assert_eq!(set_configuration(1), [0, 9, 1, 0, 0, 0, 0, 0]);
        assert_eq!(set_protocol(0, true), [0x21, 0x0B, 0, 0, 0, 0, 0, 0]);
        assert_eq!(set_protocol(2, false), [0x21, 0x0B, 1, 0, 2, 0, 0, 0]);
        assert_eq!(set_idle(0), [0x21, 0x0A, 0, 0, 0, 0, 0, 0]);
        assert!(reads(&get_descriptor(DEVICE, 0, 8)));
        assert!(!reads(&set_idle(0)));
        assert_eq!(clear_endpoint_halt(0x81), [0x02, 1, 0, 0, 0x81, 0, 0, 0]);
        assert!(!reads(&clear_endpoint_halt(0x81)));
        assert_eq!(length(&get_descriptor(CONFIGURATION, 0, 0x122)), 0x122);
    }

    /// QEMU's usb-kbd device descriptor.
    const KEYBOARD_DEVICE: [u8; 18] = [
        18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x27, 0x06, 0x01, 0x00, 0, 0, 1, 4, 11, 1,
    ];

    #[test]
    fn device_descriptors_parse() {
        let device = Device::parse(&KEYBOARD_DEVICE).unwrap();
        assert_eq!(device.usb, 0x0200);
        assert_eq!(device.max_packet0, 64);
        assert_eq!(device.vendor, 0x0627);
        assert_eq!(device.product, 0x0001);
        assert_eq!(device.configurations, 1);
        assert_eq!(Device::max_packet0(&KEYBOARD_DEVICE[..8]), Ok(64));
        // USB 3 gives the size as a power of two.
        let mut super_speed = KEYBOARD_DEVICE;
        super_speed[3] = 0x03;
        super_speed[7] = 9;
        assert_eq!(Device::max_packet0(&super_speed), Ok(512));
        assert_eq!(
            Device::parse(&KEYBOARD_DEVICE[..10]),
            Err(Error::BadDescriptor)
        );
        assert_eq!(
            Device::max_packet0(&[8, 2, 0, 2, 0, 0, 0, 8]),
            Err(Error::BadDescriptor)
        );
    }

    /// QEMU's usb-kbd configuration: one boot keyboard interface.
    const KEYBOARD_CONFIG: [u8; 34] = [
        9, 2, 34, 0, 1, 1, 0, 0xA0, 50, // configuration 1
        9, 4, 0, 0, 1, 3, 1, 1, 0, // interface 0: HID, boot, keyboard
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0, // HID 1.11, report descriptor 63 bytes
        7, 5, 0x81, 3, 8, 0, 7, // endpoint 1 IN, interrupt, 8 bytes, 7 ms
    ];

    #[test]
    fn a_boot_keyboard_configuration_parses() {
        assert_eq!(configuration_length(&KEYBOARD_CONFIG[..9]), Ok(34));
        let config = Configuration::parse(&KEYBOARD_CONFIG).unwrap();
        assert_eq!(config.value, 1);
        let interface = *config.interfaces().next().unwrap();
        assert_eq!(
            interface,
            HidInterface {
                number: 0,
                subclass: 1,
                protocol: 1,
                report_length: 63,
                endpoint: 0x81,
                max_packet: 8,
                interval: 7,
            }
        );
        assert!(interface.boot_keyboard());
        assert!(!interface.boot_mouse());
        assert_eq!(config.interfaces().count(), 1);
    }

    #[test]
    fn non_hid_interfaces_alternate_settings_and_out_endpoints_are_skipped() {
        let bytes = [
            9, 2, 57, 0, 2, 1, 0, 0x80, 50, // configuration 1, two interfaces
            9, 4, 0, 0, 1, 8, 6, 0x50, 0, // mass storage
            7, 5, 0x81, 2, 0, 2, 0, // bulk IN
            9, 4, 1, 0, 2, 3, 0, 0, 0, // HID interface 1, no boot protocol
            9, 0x21, 0x11, 0x01, 0, 1, 0x22, 74, 0, //
            7, 5, 0x02, 3, 8, 0, 10, // interrupt OUT: skipped
            7, 5, 0x83, 3, 6, 0, 10, // interrupt IN 3
        ];
        let config = Configuration::parse(&bytes).unwrap();
        let interface = config.interfaces().next().unwrap();
        assert_eq!(interface.number, 1);
        assert_eq!(interface.endpoint, 0x83);
        assert_eq!(interface.report_length, 74);
        assert!(!interface.boot_keyboard() && !interface.boot_mouse());
        assert_eq!(config.interfaces().count(), 1);
    }

    #[test]
    fn a_descriptor_running_past_the_end_is_rejected() {
        let mut bytes = KEYBOARD_CONFIG;
        bytes[27] = 9;
        assert_eq!(Configuration::parse(&bytes), Err(Error::BadDescriptor));
        bytes[27] = 0;
        assert_eq!(Configuration::parse(&bytes), Err(Error::BadDescriptor));
        assert_eq!(configuration_length(&[9, 4, 0]), Err(Error::BadDescriptor));
    }

    #[test]
    fn hub_requests_encode_as_the_hub_class_says() {
        assert_eq!(get_hub_descriptor(71), [0xA0, 6, 0, 0x29, 0, 0, 71, 0]);
        assert_eq!(get_port_status(3), [0xA3, 0, 0, 0, 3, 0, 4, 0]);
        assert_eq!(set_port_feature(2, PORT_POWER), [0x23, 3, 8, 0, 2, 0, 0, 0]);
        assert_eq!(set_port_feature(2, PORT_RESET), [0x23, 3, 4, 0, 2, 0, 0, 0]);
        assert_eq!(
            clear_port_feature(2, C_PORT_RESET),
            [0x23, 1, 20, 0, 2, 0, 0, 0]
        );
        assert!(reads(&get_port_status(1)) && !reads(&set_port_feature(1, PORT_POWER)));
    }

    #[test]
    fn a_hub_descriptor_gives_its_ports_power_good_time_and_think_time() {
        // QEMU's usb-hub (hw/usb/dev-hub.c): 8 ports, so bLength is 10, and
        // 2 ms to power good.
        let qemu = [10, 0x29, 8, 0x0A, 0x00, 1, 0, 0x00, 0xFF, 0xFF];
        let hub = HubDescriptor::parse(&qemu).unwrap();
        assert_eq!((hub.ports, hub.power_good_ms, hub.think_time), (8, 2, 0));
        // The first 7 bytes are enough, whatever bLength says of the rest.
        assert_eq!(HubDescriptor::parse(&qemu[..7]), Ok(hub));
        // Think time is bits 6:5 of the characteristics.
        let mut slow = qemu;
        slow[3] = 0x60;
        assert_eq!(HubDescriptor::parse(&slow).unwrap().think_time, 3);
        // No ports, a wrong type, and one that says it is under 7 bytes are refused.
        let mut none = qemu;
        none[2] = 0;
        assert_eq!(HubDescriptor::parse(&none), Err(Error::BadDescriptor));
        let mut wrong = qemu;
        wrong[1] = 0x2A;
        assert_eq!(HubDescriptor::parse(&wrong), Err(Error::BadDescriptor));
        assert_eq!(HubDescriptor::parse(&qemu[..6]), Err(Error::BadDescriptor));
        let mut short = qemu;
        short[0] = 6;
        assert_eq!(HubDescriptor::parse(&short), Err(Error::BadDescriptor));
    }

    #[test]
    fn a_port_status_reads_its_bits_and_its_speed() {
        use crate::regs::speed;
        // Connected, enabled, powered, high speed; the reset change is set.
        let status = PortStatus::parse(&[0x03, 0x05, 0x10, 0x00]).unwrap();
        assert!(status.connected() && status.enabled() && status.reset_done());
        assert_eq!(status.speed(), speed::HIGH);
        let low = PortStatus::parse(&[0x03, 0x03, 0, 0]).unwrap();
        assert_eq!(low.speed(), speed::LOW);
        assert!(!low.reset_done());
        let full = PortStatus::parse(&[0x01, 0x01, 0, 0]).unwrap();
        assert_eq!((full.speed(), full.enabled()), (speed::FULL, false));
        assert_eq!(PortStatus::parse(&[1, 0, 0]), Err(Error::BadDescriptor));
    }
}
