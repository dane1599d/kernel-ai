//! Calls that pass a request and a reply buffer: to a service, or over HTTP.

/// Largest request or reply passed through a service endpoint.
pub const MAX_MESSAGE: usize = 8192;

/// Argument of [`crate::call::CALL`].
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CallRequest {
    pub request_ptr: u64,
    pub request_len: u64,
    pub reply_ptr: u64,
    pub reply_cap: u64,
    pub timeout_ms: u64,
}

/// Argument of [`crate::call::HTTP`]. The kernel sends the request to the LLM
/// server named on its command line and nowhere else: ring 3 never chooses the
/// destination.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct HttpRequest {
    pub request_ptr: u64,
    pub request_len: u64,
    pub response_ptr: u64,
    pub response_cap: u64,
    pub timeout_ms: u64,
}
