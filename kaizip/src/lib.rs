//! Zip files for Files, the host half (plan P6 M-7, groundwork G14): an
//! archive is listed, extracted and made through zip 9.8.2, behind ceilings
//! a hostile archive cannot pass.
//!
//! - **Zip bombs.** An archive is refused before a byte is extracted when:
//!   - it holds more than [`Limits::max_entries`] entries;
//!   - any entry says it is over [`Limits::max_entry`];
//!   - its entries say more than [`Limits::max_total`] together;
//!   - any entry inflates more than [`Limits::max_ratio`] times its
//!     compressed size, once past [`Limits::ratio_floor`];
//!   - any two entries' data overlap (the overlapping-file bomb).
//!
//!   While extracting, every entry's real bytes are counted against what it
//!   said, and the total against [`Limits::max_total`], so an entry that lies
//!   about its size stops there.
//! - **Path escapes.** A name is refused unless every part of it is a plain
//!   name ([`names`]): no `..` or `.`, no leading `/`, no drive letter or any
//!   `:`, no `\`, no control byte, no empty part, at most
//!   [`Limits::max_depth`] deep. Symbolic links and other special entries
//!   are refused, never followed or made.
//! - **Duplicates.** Two entries with one name, or a file with an entry under
//!   it, are refused. zip keeps one entry a name, so a duplicate the reader
//!   would hide is refused too.
//! - **Encrypted entries** are refused by name ([`Error::Encrypted`]): no
//!   password is ever asked for. Only stored and deflate entries are read;
//!   any other method is refused by name ([`Error::Method`]).
//!
//! Whole archives are refused, never partly opened: Files shows the reason.
//! Extraction goes to an [`Unpack`] (Files' folder, or [`Tree`] in memory);
//! [`Maker`] makes an archive under the same rules.

mod limits;
mod make;
pub mod names;
mod read;

pub use limits::{Error, Limits};
pub use make::Maker;
pub use read::{Archive, Entry, Kind, Tree, Unpack};
