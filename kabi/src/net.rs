//! Limits of [`super::call::RESOLVE`], and the stream calls' flags (P5 task 2).

/// The longest host name, without a trailing dot.
pub const MAX_HOST: usize = 253;
/// The most addresses one answer gives.
pub const MAX_ADDRESSES: usize = 8;
/// How long the kernel tries before the call times out.
pub const RESOLVE_TIMEOUT_MS: u64 = 5_000;

/// [`super::call::NET_CONNECT`]'s flag: a bulk stream, with a 1 MiB receive
/// buffer (at most 4 at once in all).
pub const STREAM_BULK: u64 = 1 << 0;
/// The most bytes one [`super::call::NET_SEND`] takes or one
/// [`super::call::NET_RECV`] gives.
pub const STREAM_MAX_CALL: usize = 8 * 1024;
/// Streams one process may hold open, and in all.
pub const STREAMS_PER_PROCESS: usize = 16;
pub const STREAMS_MAX: usize = 64;

/// What [`super::call::NET_POLL`] reports and waits for.
pub const STREAM_CONNECTED: u64 = 1 << 0;
pub const STREAM_READABLE: u64 = 1 << 1;
pub const STREAM_WRITABLE: u64 = 1 << 2;
/// The peer closed, and everything it sent has been read.
pub const STREAM_CLOSED: u64 = 1 << 3;
/// It never connected: refused, unreachable or timed out.
pub const STREAM_FAILED: u64 = 1 << 4;
