//! CLOCK_NOW: the date and time for any process ([`crate::clock`]), so the
//! shell can answer "what time is it" (founder test 2, item 12).

use kabi::{Error, call};

/// [`call::CLOCK_NOW`], or `None` for a number this area does not have.
pub(super) fn handle(_slot: usize, number: u64, _a: &[u64; 6]) -> Option<Result<u64, Error>> {
    (number == call::CLOCK_NOW).then(|| crate::clock::unix_seconds().ok_or(Error::NotFound))
}
