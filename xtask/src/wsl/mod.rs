//! Kernel AI and WSL2 on Windows: what a distro offers (`probe.rs`), the
//! checklist for it (`check.rs`), and `--accel wsl`, which runs QEMU in it
//! (`launch.rs`, with the paths and memory in `path.rs`).

pub mod check;
#[cfg(test)]
pub(crate) mod check_tests;
mod launch;
mod path;
pub mod probe;
#[cfg(test)]
mod tests;

pub use launch::{Wsl, command, firmware, wrap};
