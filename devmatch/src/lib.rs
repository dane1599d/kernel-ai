//! Which driver takes a PCI function (ADR 0007 section 3): one compiled-in table
//! in place of the arms that `virtio::pci::probe` had, so the kernel stays a
//! static binary. An `Id` row beats a `Class` or `Virtio` row for the same
//! function. `drv=` on the kernel command line moves or disables a driver for
//! bring-up on new hardware.

#![no_std]

/// The PCI vendor of every virtio device.
pub const VIRTIO_VENDOR: u16 = 0x1af4;

/// What a row matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Class, subclass and programming interface.
    Class(u8, u8, u8),
    /// Vendor and device.
    Id(u16, u16),
    /// A virtio device type (1 network, 2 block, 16 GPU, ...), from a modern
    /// (0x1040 + type) or transitional device ID.
    Virtio(u16),
}

/// Where a driver runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Kernel,
    User {
        service: &'static str,
        program: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: Key,
    pub driver: &'static str,
    pub place: Place,
    /// The supervisor never gives up on its service.
    pub essential: bool,
    /// It feeds the input queue.
    pub input: bool,
}

const fn kernel(key: Key, driver: &'static str) -> Row {
    Row {
        key,
        driver,
        place: Place::Kernel,
        essential: false,
        input: false,
    }
}

/// The drivers built into the kernel today.
pub const TABLE: &[Row] = &[
    kernel(Key::Virtio(1), "virtio-net"),
    kernel(Key::Virtio(2), "virtio-blk"),
    kernel(Key::Virtio(16), "virtio-gpu"),
    kernel(Key::Class(0x01, 0x08, 0x02), "nvme"),
    Row {
        input: true,
        ..kernel(Key::Class(0x0c, 0x03, 0x30), "xhci")
    },
];

/// The identity of a function, as the match sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub vendor: u16,
    pub device: u16,
    pub class: (u8, u8, u8),
}

impl Ids {
    /// The virtio device type, for a virtio function.
    pub fn virtio_type(&self) -> Option<u16> {
        if self.vendor != VIRTIO_VENDOR {
            return None;
        }
        match self.device {
            0x1040..=0x107f => Some(self.device - 0x1040),
            // Transitional devices (virtio 1.2 section 4.1.2.1).
            0x1000 => Some(1),
            0x1001 => Some(2),
            0x1002 => Some(5),
            0x1003 => Some(3),
            0x1004 => Some(8),
            0x1005 => Some(4),
            0x1009 => Some(9),
            _ => None,
        }
    }

    fn matches(&self, key: Key) -> bool {
        match key {
            Key::Id(vendor, device) => (self.vendor, self.device) == (vendor, device),
            Key::Class(a, b, c) => self.class == (a, b, c),
            Key::Virtio(kind) => self.virtio_type() == Some(kind),
        }
    }
}

/// The row for `ids` in `table`: an `Id` row first, else the first other match.
pub fn lookup(table: &'static [Row], ids: Ids) -> Option<&'static Row> {
    let by_id = |row: &&Row| matches!(row.key, Key::Id(..)) && ids.matches(row.key);
    table
        .iter()
        .find(by_id)
        .or_else(|| table.iter().find(|row| ids.matches(row.key)))
}

/// A `drv=` entry: where to run a driver, overriding its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Override {
    Kernel,
    User,
    Off,
}

/// The override for `driver` in a `drv=name:place,...` value; the last entry
/// for a driver wins. Entries that do not parse are ignored.
pub fn override_for(value: &str, driver: &str) -> Option<Override> {
    value
        .rsplit(',')
        .filter_map(|entry| entry.split_once(':'))
        .filter(|(name, _)| *name == driver)
        .find_map(|(_, place)| match place {
            "kernel" => Some(Override::Kernel),
            "user" => Some(Override::User),
            "off" => Some(Override::Off),
            _ => None,
        })
}

/// The value of `drv=` on a kernel command line, or "".
pub fn drv_value(cmdline: &str) -> &str {
    cmdline
        .split_whitespace()
        .rev()
        .find_map(|arg| arg.strip_prefix("drv="))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ids(vendor: u16, device: u16, class: (u8, u8, u8)) -> Ids {
        Ids {
            vendor,
            device,
            class,
        }
    }

    fn driver(ids: Ids) -> Option<&'static str> {
        lookup(TABLE, ids).map(|row| row.driver)
    }

    #[test]
    fn todays_devices_get_todays_drivers() {
        assert_eq!(driver(ids(0x1af4, 0x1041, (2, 0, 0))), Some("virtio-net"));
        assert_eq!(driver(ids(0x1af4, 0x1000, (2, 0, 0))), Some("virtio-net"));
        assert_eq!(driver(ids(0x1af4, 0x1001, (1, 0, 0))), Some("virtio-blk"));
        assert_eq!(driver(ids(0x1af4, 0x1042, (1, 0, 0))), Some("virtio-blk"));
        assert_eq!(
            driver(ids(0x1af4, 0x1050, (3, 0x80, 0))),
            Some("virtio-gpu")
        );
        assert_eq!(driver(ids(0x1b36, 0x0010, (1, 8, 2))), Some("nvme"));
        assert_eq!(driver(ids(0x1b36, 0x000d, (0x0c, 3, 0x30))), Some("xhci"));
        assert!(
            lookup(TABLE, ids(0x1b36, 0x000d, (0x0c, 3, 0x30)))
                .unwrap()
                .input
        );
    }

    #[test]
    fn other_devices_get_none() {
        assert_eq!(driver(ids(0x8086, 0x29c0, (6, 0, 0))), None);
        // A virtio device type we have no driver for, and a non-virtio vendor
        // with a virtio-like device ID.
        assert_eq!(driver(ids(0x1af4, 0x1044, (0xff, 0, 0))), None);
        assert_eq!(driver(ids(0x10ec, 0x1041, (2, 0, 0))), None);
        // An xHCI-like class with another programming interface (EHCI).
        assert_eq!(driver(ids(0x8086, 0x293a, (0x0c, 3, 0x20))), None);
    }

    #[test]
    fn an_id_row_beats_a_class_row() {
        static TABLE: &[Row] = &[
            kernel(Key::Class(0x0c, 0x03, 0x30), "xhci"),
            kernel(Key::Id(0x1022, 0x43f7), "xhci-quirky"),
        ];
        assert_eq!(
            lookup(TABLE, ids(0x1022, 0x43f7, (0x0c, 3, 0x30))).map(|r| r.driver),
            Some("xhci-quirky")
        );
        assert_eq!(
            lookup(TABLE, ids(0x1b36, 0x000d, (0x0c, 3, 0x30))).map(|r| r.driver),
            Some("xhci")
        );
    }

    #[test]
    fn drv_moves_or_disables_drivers() {
        let line = "test=net drv=usbd:kernel,netd:off,xhci:off,bad,nvme:nowhere drv=usbd:user";
        let value = drv_value(line);
        assert_eq!(value, "usbd:user");
        let value = "usbd:kernel,netd:off,xhci:off,bad,nvme:nowhere,xhci:kernel";
        assert_eq!(override_for(value, "usbd"), Some(Override::Kernel));
        assert_eq!(override_for(value, "netd"), Some(Override::Off));
        assert_eq!(override_for(value, "xhci"), Some(Override::Kernel));
        assert_eq!(override_for(value, "nvme"), None);
        assert_eq!(override_for("", "nvme"), None);
        assert_eq!(drv_value("test=net"), "");
    }
}
