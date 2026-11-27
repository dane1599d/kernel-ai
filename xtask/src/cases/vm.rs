//! P6 row 3's exit case: a tiny guest writes `vm-hello` to COM1 (ADR 0006).
//!
//! Written ahead of the hypervisor's run loop and **not run**: it is in no lane's
//! table, so `cargo xtask test` never boots it. The run loop's PR adds it to
//! [`super::LANES`], and the kernel's `test=vm-hello` mode loads
//! [`crate::vmguest::image`] at [`crate::vmguest::LOAD`] and runs it.

use super::{TestCase, case};

/// What the kernel logs for each line the guest writes to COM1. The case's own
/// command line holds `vm-hello` too, so the line carries more than that.
pub(crate) const GUEST_LINE: &str = "vm-hello: guest serial: vm-hello";

#[allow(dead_code)] // Not run until the run loop lands; see the module doc.
pub(crate) const VM_HELLO: TestCase = TestCase {
    expect: &[GUEST_LINE],
    ..case("vm-hello", "test=vm-hello")
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_case_is_written_and_xtasks_list_does_not_run_it() {
        assert!(VM_HELLO.cmdline.contains("vm-hello"));
        assert!(GUEST_LINE.ends_with(crate::vmguest::EXPECTED_SERIAL));
        assert!(
            super::super::test_cases()
                .iter()
                .all(|case| case.name != VM_HELLO.name)
        );
    }
}
