//! Spike S04-2 (ADR 0004): what the stub `fs` and its client `fsbench` say to each
//! other. File requests are shaped like `kai-fs 1` (ADR 0004 §6): a CBOR array
//! `[version, op, a, b, data]` answered by `[status, value, data]`, with at most
//! 7 KiB of data a message. The raw requests carry bare bytes, to time the copied
//! IPC alone. Included by both programs; built only on the never-merged branch
//! spike/fs-ipc.

// Each program uses only part of it.
#![allow(dead_code)]

/// The version every CBOR request carries.
pub const VERSION: u32 = 1;

/// Raw: the request's 8 KiB come back unchanged.
pub const RAW_ECHO: u8 = b'E';
/// Raw: an 8 KiB request, answered with [`RAW_SMALL`] bytes.
pub const RAW_SINK: u8 = b'S';
/// Raw: a [`RAW_SMALL`]-byte request, answered with 8 KiB.
pub const RAW_SOURCE: u8 = b'R';
/// The small side of a one-way raw call.
pub const RAW_SMALL: usize = 16;

/// `a` = file length, `b` = the checksum kept for each record as it fills.
/// Makes the file anew, zeroed.
pub const OP_OPEN: u32 = 1;
/// `a` = offset; `data` = the bytes. Answered once the bytes are in the cache and
/// every record they complete has its checksum.
pub const OP_WRITE: u32 = 2;
/// `a` = offset, `b` = length (at most [`DATA_MAX`]); the reply's `data`. With a
/// verify kind set, each record the read starts is checksummed and compared
/// first, as `fs` would when it loads a record from the disk.
pub const OP_READ: u32 = 3;
/// `b` = the checksum to verify reads with ([`CSUM_NONE`] for a warm cache).
pub const OP_VERIFY: u32 = 4;
/// The reply's `value` is seahash-64 over the whole file, to check its bytes.
pub const OP_DIGEST: u32 = 5;
/// `b` = a checksum kind; the reply's `value` is the nanoseconds it took over
/// the whole file in 128 KiB records, inside `fs`.
pub const OP_CSUM_BENCH: u32 = 6;
/// The server answers and exits.
pub const OP_QUIT: u32 = 9;

pub const CSUM_NONE: u64 = 0;
pub const CSUM_SEAHASH: u64 = 1;
pub const CSUM_SHA256: u64 = 2;

/// Most data in one `kai-fs 1` message (ADR 0004 §6).
pub const DATA_MAX: usize = 7 * 1024;
/// A kaifs data record (ADR 0004 §1).
pub const RECORD: usize = 128 * 1024;

/// Status 0 is success; anything else names what went wrong.
pub const OK: u32 = 0;
pub const BAD_REQUEST: u32 = 1;
pub const OUT_OF_RANGE: u32 = 2;
pub const BAD_CHECKSUM: u32 = 3;

pub fn csum_name(kind: u64) -> &'static str {
    match kind {
        CSUM_SEAHASH => "seahash-64",
        CSUM_SHA256 => "SHA-256",
        _ => "none",
    }
}
