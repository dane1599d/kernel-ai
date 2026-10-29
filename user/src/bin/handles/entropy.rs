//! ENTROPY from ring 3, with no capability (as getrandom): two draws differ and
//! are not all zero, an empty draw is fine, and a request over the limit is
//! refused whole.

use kabi::Error;
use user::{println, sys};

pub fn check() -> Result<(), &'static str> {
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    sys::entropy(&mut a).map_err(|_| "ENTROPY failed")?;
    sys::entropy(&mut b).map_err(|_| "a second ENTROPY failed")?;
    if a == b || a == [0; 32] || b == [0; 32] {
        return Err("ENTROPY gave the same or empty bytes");
    }
    if sys::entropy(&mut []) != Ok(0) {
        return Err("an empty ENTROPY did not answer 0");
    }
    let mut most = [0u8; kabi::call::ENTROPY_MAX];
    if sys::entropy(&mut most) != Ok(most.len()) {
        return Err("ENTROPY refused its largest request");
    }
    let mut over = [0u8; kabi::call::ENTROPY_MAX + 1];
    super::expect(
        sys::entropy(&mut over),
        Error::TooBig,
        "an ENTROPY over the limit",
    )?;
    if over != [0u8; kabi::call::ENTROPY_MAX + 1] {
        return Err("a refused ENTROPY wrote bytes");
    }
    println!(
        "handles: ENTROPY gave two different draws with no capability, and refused {} bytes",
        over.len()
    );
    Ok(())
}
