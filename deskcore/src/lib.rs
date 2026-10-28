//! `desktop`'s logic without a screen (ADR 0005 §1): the scene, damage, focus,
//! window moves and resizes, the chrome's geometry, and the scale. `desktop`
//! turns the kernel's input and the clients' `kai-surface 1` messages into calls
//! on [`desk::Desk`], carries out the [`desk::Effect`]s it answers with, and
//! draws what [`desk::Desk::take_damage`] says changed. Everything here is
//! tested on the host from recorded input.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod askcard;
pub mod chrome;
pub mod compose;
pub mod damage;
pub mod desk;
pub mod geom;
pub mod keymap;
pub mod paint;
pub mod paint_card;
pub mod paint_panel;
pub mod panel;
pub mod scale;
pub mod server;
pub mod trusted;
pub mod watch;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod server_tests;

#[cfg(test)]
mod paint_tests;
