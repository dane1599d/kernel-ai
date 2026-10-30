//! ENTROPY: random bytes from the kernel's generator ([`crate::entropy`]), for
//! any process, like getrandom.

use crate::process::uaccess::copy_to_user;
use kabi::call::ENTROPY_MAX;
use kabi::{Error, call};

const _: () = assert!(ENTROPY_MAX == csprng::MAX_REQUEST);

/// [`call::ENTROPY`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    (number == call::ENTROPY).then(|| entropy(slot, a[0], a[1]))
}

fn entropy(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    let len = usize::try_from(len)
        .ok()
        .filter(|&len| len <= ENTROPY_MAX)
        .ok_or(Error::TooBig)?;
    let mut bytes = [0u8; ENTROPY_MAX];
    if !crate::entropy::fill(&mut bytes[..len]) {
        return Err(Error::NoEntropy);
    }
    let copied = copy_to_user(slot, ptr, &bytes[..len]);
    bytes.fill(0);
    copied?;
    Ok(len as u64)
}
