//! K, the kernel: faults, time, scheduling, SMP, threads and memory.
//! Its cases are split by area, and [`super::LANES`] lists the tables in this order:
//! basics, display, apps, desk, memory.

pub(super) mod apps;
pub(super) mod basics;
pub(super) mod desk;
pub(super) mod display;
pub(super) mod memory;

use super::{KAI, TestCase, case};
