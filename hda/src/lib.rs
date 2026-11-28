//! An Intel High Definition Audio controller and codec driver (ADR 0007's
//! HD Audio row, v0.3): the CORB and RIRB, codec verbs and their responses,
//! the walk of a codec's widget graph to an output path (pin, mixers and
//! selectors, DAC), and one output stream's buffer descriptor list, set up to
//! play a test tone. It is written against [`devkit::Host`], so it runs in the
//! kernel now and moves to `audiod` in ring 3 later with only its host
//! swapped. Everything is host-tested over a fake controller and codec.
//!
//! Written from Intel's High Definition Audio Specification, revision 1.0a;
//! no code was taken from another driver or emulator.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod controller;
pub mod graph;
pub mod regs;
pub mod stream;
pub mod verb;

#[cfg(test)]
mod tests;

pub use controller::{Controller, Opened, Playing, Regs, Tone};
pub use graph::{Codec, FunctionGroup, Path};
pub use stream::Format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A register did not reach the state it was waiting for.
    Timeout(&'static str),
    /// No codec answered the link reset.
    NoCodec,
    /// The codec gave no answer to a command.
    NoResponse {
        nid: u8,
        verb: u32,
    },
    NoAudioFunctionGroup,
    /// No connected output pin reaches an analogue DAC.
    NoOutputPath,
    /// GCAP reports no output stream.
    NoOutputStream,
    /// The codec said something the driver cannot follow.
    Codec(&'static str),
    Dma(devkit::DmaError),
    Unsupported(&'static str),
}
