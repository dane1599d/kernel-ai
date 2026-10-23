//! Scheduling policies for [`crate::call::POLICY`].

pub const INTERACTIVE: u64 = 0;
pub const ROUND_ROBIN: u64 = 1;
/// `classes`, with a [`crate::sched::preset`] as the call's second argument.
pub const CLASSES: u64 = 2;
/// `responsive`, the rule-based baseline.
pub const RESPONSIVE: u64 = 3;
/// In [`crate::Metrics`] only: a policy outside the catalog, such as a test's.
pub const OTHER: u64 = 255;
