//! Errors, returned in RAX as their negative value.

/// Errors, returned in RAX as their negative value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// The process lacks the capability for this call.
    Permission = 1,
    /// A pointer argument is not mapped, not user memory, or not writable.
    Fault = 2,
    Invalid = 3,
    /// The page quota or physical memory is exhausted.
    NoMemory = 4,
    TimedOut = 5,
    /// No such process, service or call.
    NotFound = 6,
    Busy = 7,
    Network = 8,
    TooBig = 9,
    /// The other side died before answering.
    Dead = 10,
    /// The user pressed Ctrl-C.
    Interrupted = 11,
    /// The service is not running.
    Unavailable = 12,
}

impl Error {
    pub const ALL: [Error; 12] = [
        Error::Permission,
        Error::Fault,
        Error::Invalid,
        Error::NoMemory,
        Error::TimedOut,
        Error::NotFound,
        Error::Busy,
        Error::Network,
        Error::TooBig,
        Error::Dead,
        Error::Interrupted,
        Error::Unavailable,
    ];

    /// The value placed in RAX.
    pub const fn to_raw(self) -> u64 {
        (-(self as i64)) as u64
    }

    /// Splits a raw result into a value or an error.
    pub fn from_raw(raw: u64) -> Result<u64, Error> {
        let signed = raw as i64;
        if signed >= 0 {
            return Ok(raw);
        }
        let known = Self::ALL
            .into_iter()
            .find(|error| (*error as i64).checked_neg() == Some(signed));
        Err(known.unwrap_or(Error::Invalid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_round_trip_through_rax() {
        for error in Error::ALL {
            assert_eq!(Error::from_raw(error.to_raw()), Err(error));
        }
        assert_eq!(Error::from_raw(42), Ok(42));
        assert_eq!(Error::from_raw(u64::MAX - 999), Err(Error::Invalid));
    }
}
