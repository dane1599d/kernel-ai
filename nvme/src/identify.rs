//! The parts of the Identify Controller and Identify Namespace pages the driver
//! uses, and the active namespace list.

/// Identify data is one 4 KiB page.
pub const IDENTIFY_BYTES: usize = 4096;

/// From Identify Controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controller {
    /// The serial number, trailing spaces trimmed (QEMU's `serial=`).
    pub serial: [u8; 20],
    pub serial_len: usize,
    /// The model number, trimmed the same way.
    pub model: [u8; 40],
    pub model_len: usize,
    /// Largest transfer as a power of two of the minimum page size; 0 is no limit.
    pub mdts: u8,
    /// Namespaces the controller may have (NN).
    pub namespaces: u32,
}

/// Why an Identify page cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifyError {
    Short,
    /// FLBAS names a format the namespace does not list.
    NoFormat,
}

fn trimmed<const N: usize>(bytes: &[u8]) -> ([u8; N], usize) {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[..N]);
    let len = out
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |last| last + 1);
    (out, len)
}

fn u32_at(page: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]])
}

fn u64_at(page: &[u8], at: usize) -> u64 {
    u64::from(u32_at(page, at)) | (u64::from(u32_at(page, at + 4)) << 32)
}

impl Controller {
    pub fn parse(page: &[u8]) -> Result<Self, IdentifyError> {
        if page.len() < IDENTIFY_BYTES {
            return Err(IdentifyError::Short);
        }
        let (serial, serial_len) = trimmed::<20>(&page[4..24]);
        let (model, model_len) = trimmed::<40>(&page[24..64]);
        Ok(Self {
            serial,
            serial_len,
            model,
            model_len,
            mdts: page[77],
            namespaces: u32_at(page, 516),
        })
    }

    pub fn serial(&self) -> &[u8] {
        &self.serial[..self.serial_len]
    }

    pub fn model(&self) -> &[u8] {
        &self.model[..self.model_len]
    }

    /// The most bytes one command may move, for pages of `1 << page_shift`
    /// bytes, capped at `cap`.
    pub fn max_transfer(&self, page_shift: u32, cap: u64) -> u64 {
        match self.mdts {
            0 => cap,
            mdts => (1u64 << (page_shift + u32::from(mdts).min(31))).min(cap),
        }
    }
}

/// From Identify Namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace {
    /// Size in logical blocks (NSZE).
    pub blocks: u64,
    /// Bytes in a logical block of the format in use.
    pub block_bytes: u32,
    /// The namespace is write-protected (NSATTR bit 0).
    pub write_protected: bool,
}

impl Namespace {
    pub fn parse(page: &[u8]) -> Result<Self, IdentifyError> {
        if page.len() < IDENTIFY_BYTES {
            return Err(IdentifyError::Short);
        }
        let formats = usize::from(page[25]) + 1;
        // FLBAS bits 0..4, and bits 5..6 for formats past 16.
        let flbas = page[26];
        let format = usize::from(flbas & 0xF) | (usize::from((flbas >> 5) & 0x3) << 4);
        if format >= formats {
            return Err(IdentifyError::NoFormat);
        }
        let lbaf = u32_at(page, 128 + 4 * format);
        let lbads = (lbaf >> 16) & 0xFF;
        if !(9..=16).contains(&lbads) {
            return Err(IdentifyError::NoFormat);
        }
        Ok(Self {
            blocks: u64_at(page, 0),
            block_bytes: 1 << lbads,
            write_protected: page[99] & 1 == 1,
        })
    }
}

/// The active namespace IDs in a CNS 2 page: ascending, ended by the first zero.
pub fn active_namespaces(page: &[u8]) -> impl Iterator<Item = u32> + '_ {
    page.as_chunks::<4>()
        .0
        .iter()
        .take(IDENTIFY_BYTES / 4)
        .map(|id| u32::from_le_bytes(*id))
        .take_while(|&id| id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> [u8; IDENTIFY_BYTES] {
        [0; IDENTIFY_BYTES]
    }

    #[test]
    fn a_controller_page_gives_serial_model_mdts_and_nn() {
        let mut id = page();
        id[4..24].copy_from_slice(b"kai-model           ");
        id[24..64].copy_from_slice(b"QEMU NVMe Ctrl                          ");
        id[77] = 7;
        id[516..520].copy_from_slice(&256u32.to_le_bytes());
        let controller = Controller::parse(&id).unwrap();
        assert_eq!(controller.serial(), b"kai-model");
        assert_eq!(controller.model(), b"QEMU NVMe Ctrl");
        assert_eq!(controller.namespaces, 256);
        assert_eq!(controller.max_transfer(12, 1 << 20), 512 * 1024);
        assert_eq!(controller.max_transfer(12, 128 * 1024), 128 * 1024);
        id[77] = 0;
        let unlimited = Controller::parse(&id).unwrap();
        assert_eq!(unlimited.max_transfer(12, 1 << 20), 1 << 20);
        assert_eq!(Controller::parse(&id[..100]), Err(IdentifyError::Short));
    }

    #[test]
    fn a_namespace_page_gives_size_and_block_size() {
        let mut id = page();
        id[0..8].copy_from_slice(&6145u64.to_le_bytes());
        id[25] = 1; // two formats
        id[26] = 1; // the second in use
        id[128..132].copy_from_slice(&(9u32 << 16).to_le_bytes());
        id[132..136].copy_from_slice(&(12u32 << 16).to_le_bytes());
        let ns = Namespace::parse(&id).unwrap();
        assert_eq!(ns.blocks, 6145);
        assert_eq!(ns.block_bytes, 4096);
        assert!(!ns.write_protected);
        id[26] = 0;
        id[99] = 1;
        let ns = Namespace::parse(&id).unwrap();
        assert_eq!(ns.block_bytes, 512);
        assert!(ns.write_protected);
        id[26] = 2;
        assert_eq!(Namespace::parse(&id), Err(IdentifyError::NoFormat));
    }

    #[test]
    fn the_active_list_stops_at_zero() {
        let mut id = page();
        id[0..4].copy_from_slice(&1u32.to_le_bytes());
        id[4..8].copy_from_slice(&2u32.to_le_bytes());
        let ids: [u32; 2] = [1, 2];
        assert!(active_namespaces(&id).eq(ids));
        assert_eq!(active_namespaces(&page()).count(), 0);
    }
}
