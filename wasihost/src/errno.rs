//! The WASI preview-1 error numbers this crate returns (`wasi_snapshot_preview1`'s
//! `errno` enum, by value).

/// A WASI error number; 0 is success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub u16);

impl Errno {
    pub const SUCCESS: Self = Self(0);
    pub const BADF: Self = Self(8);
    pub const EXIST: Self = Self(20);
    pub const FAULT: Self = Self(21);
    pub const FBIG: Self = Self(22);
    pub const INVAL: Self = Self(28);
    pub const ISDIR: Self = Self(31);
    pub const MFILE: Self = Self(33);
    pub const NAMETOOLONG: Self = Self(37);
    pub const NOENT: Self = Self(44);
    pub const NOSPC: Self = Self(51);
    pub const NOSYS: Self = Self(52);
    pub const NOTDIR: Self = Self(54);
    pub const NOTEMPTY: Self = Self(55);
    pub const NOTSUP: Self = Self(58);
    pub const ROFS: Self = Self(69);
    pub const SPIPE: Self = Self(70);
    pub const NOTCAPABLE: Self = Self(76);
}

/// What a call returns to the glue: an error number, or `Err` for an error number
/// built with `?`.
pub type Res = Result<(), Errno>;
