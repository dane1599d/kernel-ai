//! Limits of [`super::call::RESOLVE`].

/// The longest host name, without a trailing dot.
pub const MAX_HOST: usize = 253;
/// The most addresses one answer gives.
pub const MAX_ADDRESSES: usize = 8;
/// How long the kernel tries before the call times out.
pub const RESOLVE_TIMEOUT_MS: u64 = 5_000;
