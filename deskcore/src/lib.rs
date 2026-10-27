//! `desktop`'s logic without a screen (ADR 0005 §1): the scene, damage, focus,
//! window moves and resizes, the chrome's geometry, and the scale. `desktop`
//! turns the kernel's input and the clients' `kai-surface 1` messages into calls
//! on [`desk::Desk`], carries out the [`desk::Effect`]s it answers with, and
//! draws what [`desk::Desk::take_damage`] says changed. Everything here is
//! tested on the host from recorded input.

#![no_std]

extern crate alloc;

pub mod chrome;
pub mod damage;
pub mod desk;
pub mod geom;
pub mod scale;

#[cfg(test)]
mod tests;
