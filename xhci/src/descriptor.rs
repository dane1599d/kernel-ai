//! USB standard and HID class requests, and the descriptors the driver reads:
//! the device descriptor and the configuration with its interfaces, HID
//! descriptors and endpoints.

use crate::Error;

/// An 8-byte SETUP packet.
pub type Request = [u8; 8];

const GET_DESCRIPTOR: u8 = 6;
const SET_CONFIGURATION: u8 = 9;
const SET_IDLE: u8 = 0x0A;
const SET_PROTOCOL: u8 = 0x0B;

pub const DEVICE: u8 = 1;
pub const CONFIGURATION: u8 = 2;
pub const INTERFACE: u8 = 4;
pub const ENDPOINT: u8 = 5;
pub const HID: u8 = 0x21;
pub const REPORT: u8 = 0x22;

pub const DEVICE_BYTES: u16 = 18;
pub const CONFIGURATION_BYTES: u16 = 9;

pub const CLASS_HID: u8 = 3;

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
}
