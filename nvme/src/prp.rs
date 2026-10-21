//! Physical Region Page entries for a transfer into physically contiguous memory.
//!
//! PRP1 points at the first byte and may be anywhere in its page; every later
//! entry is page-aligned. A transfer touching two pages puts the second page in
//! PRP2; one touching more puts the address of a list of the remaining pages in
//! PRP2. The driver's buffers are contiguous, so the list is just consecutive
//! pages, and one 4 KiB list page (512 entries) covers 2 MiB, well past what one
//! read moves.

use crate::PAGE;

/// Entries one list page holds; the driver never chains lists.
pub const LIST_ENTRIES: usize = (PAGE / 8) as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prp {
    pub prp1: u64,
    /// Zero, the second page, or the list's address.
    pub prp2: u64,
    /// How many list entries the driver must write (zero without a list).
    pub list_len: usize,
}

/// Why a transfer cannot be described.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrpError {
    Empty,
    /// More pages than one list page holds.
    TooLong,
    /// The list page itself is not page-aligned.
    UnalignedList,
}

/// The PRPs for `len` bytes at physical `phys`, with the list (if one is needed)
/// at the page `list`. Write [`list_entry`] `0..list_len` into it.
pub fn describe(phys: u64, len: u64, list: u64) -> Result<Prp, PrpError> {
    if len == 0 {
        return Err(PrpError::Empty);
    }
    let first_page = phys & !(PAGE - 1);
    let pages = (phys + len - first_page).div_ceil(PAGE);
    match pages {
        1 => Ok(Prp {
            prp1: phys,
            prp2: 0,
            list_len: 0,
        }),
        2 => Ok(Prp {
            prp1: phys,
            prp2: first_page + PAGE,
            list_len: 0,
        }),
        _ if pages - 1 > LIST_ENTRIES as u64 => Err(PrpError::TooLong),
        _ if !list.is_multiple_of(PAGE) => Err(PrpError::UnalignedList),
        _ => Ok(Prp {
            prp1: phys,
            prp2: list,
            list_len: (pages - 1) as usize,
        }),
    }
}

/// List entry `index` for a transfer starting at `phys`: the page after the
/// first, then the next, and so on.
pub fn list_entry(phys: u64, index: usize) -> u64 {
    (phys & !(PAGE - 1)) + PAGE * (index as u64 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_and_two_pages_need_no_list() {
        let one = describe(0x10_0000, 512, 0x9000).unwrap();
        assert_eq!((one.prp1, one.prp2, one.list_len), (0x10_0000, 0, 0));
        let two = describe(0x10_0000, 8192, 0x9000).unwrap();
        assert_eq!((two.prp2, two.list_len), (0x10_1000, 0));
        // An offset start spills into a second page.
        let spill = describe(0x10_0E00, 1024, 0x9000).unwrap();
        assert_eq!((spill.prp1, spill.prp2), (0x10_0E00, 0x10_1000));
    }

    #[test]
    fn a_long_transfer_lists_every_later_page() {
        let prp = describe(0x20_0000, 512 * 1024, 0x9000).unwrap();
        assert_eq!(prp.prp1, 0x20_0000);
        assert_eq!(prp.prp2, 0x9000);
        assert_eq!(prp.list_len, 127);
        assert_eq!(list_entry(0x20_0000, 0), 0x20_1000);
        assert_eq!(list_entry(0x20_0000, 126), 0x20_0000 + 127 * 4096);
    }

    #[test]
    fn limits_are_refused() {
        assert_eq!(describe(0, 0, 0), Err(PrpError::Empty));
        assert_eq!(
            describe(0x20_0000, (LIST_ENTRIES as u64 + 2) * PAGE, 0x9000),
            Err(PrpError::TooLong)
        );
        assert!(describe(0x20_0000, (LIST_ENTRIES as u64 + 1) * PAGE, 0x9000).is_ok());
        assert_eq!(
            describe(0x20_0000, 3 * PAGE, 0x9008),
            Err(PrpError::UnalignedList)
        );
    }
}
