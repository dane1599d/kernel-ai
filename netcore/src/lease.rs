//! What a DHCP lease that has just arrived is to the one the interface holds.
//!
//! smoltcp reports every renewal, changed or not (it keeps each packet for the
//! lease time). One that changes nothing is only a renewal: it is not a new
//! lease, and it must not count as one (`net-setup` waits for `leases` to stay
//! at 1 while the capped lease renews). The kernel's `net::setup` asks here;
//! the tests are on the host.

use core::net::Ipv4Addr;

/// The DNS servers a lease names.
pub const MAX_DNS: usize = 3;

/// What a lease says, and what the interface was given from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub server: Ipv4Addr,
    /// The lease time after any cap; None for an endless one.
    pub lease_s: Option<u32>,
    pub address: (Ipv4Addr, u8),
    pub router: Option<Ipv4Addr>,
    pub dns: [Option<Ipv4Addr>; MAX_DNS],
}

/// What the interface held before the lease came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Was {
    /// The fallback address.
    Fallback,
    /// A lease from DHCP, as it was taken.
    Lease(Lease),
    /// Nothing, or an address from the command line.
    Else,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The first lease, or one after none.
    First,
    /// The first lease after the fallback.
    LeavingFallback,
    /// The same lease again.
    Renewed,
    /// A lease that differs from the one held.
    Changed,
}

impl Kind {
    /// Of the lease `new`, given what was held.
    pub fn of(was: Was, new: &Lease) -> Self {
        match was {
            Was::Fallback => Self::LeavingFallback,
            Was::Lease(held) if held == *new => Self::Renewed,
            Was::Lease(_) => Self::Changed,
            Was::Else => Self::First,
        }
    }

    /// Whether `Status::leases` counts it: renewals that change nothing are not.
    pub fn counts(self) -> bool {
        self != Self::Renewed
    }

    /// What the lease line ends with, after its own text.
    pub fn note(self) -> &'static str {
        match self {
            Self::First => "",
            Self::LeavingFallback => "; leaving the fallback",
            Self::Renewed => "; renewed",
            Self::Changed => "; the lease changed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> Lease {
        Lease {
            server: Ipv4Addr::new(10, 0, 2, 2),
            lease_s: Some(4),
            address: (Ipv4Addr::new(10, 0, 2, 15), 24),
            router: Some(Ipv4Addr::new(10, 0, 2, 2)),
            dns: [Some(Ipv4Addr::new(10, 0, 2, 3)), None, None],
        }
    }

    /// The kernel's tally over `arrivals`: what it counts and the last note.
    fn run(arrivals: &[Lease]) -> (u32, &'static str) {
        let (mut was, mut leases, mut note) = (Was::Else, 0, "");
        for lease in arrivals {
            let kind = Kind::of(was, lease);
            leases += u32::from(kind.counts());
            note = kind.note();
            was = Was::Lease(*lease);
        }
        (leases, note)
    }

    #[test]
    fn a_renewal_that_changes_nothing_is_noted_and_not_counted() {
        assert_eq!(run(&[lease()]), (1, ""));
        assert_eq!(run(&[lease(), lease()]), (1, "; renewed"));
        assert_eq!(run(&[lease(), lease(), lease()]), (1, "; renewed"));
    }

    #[test]
    fn a_lease_that_differs_in_any_part_is_a_change_and_counted() {
        let base = lease();
        let differs = [
            Lease {
                lease_s: Some(5),
                ..base
            },
            Lease {
                lease_s: None,
                ..base
            },
            Lease {
                server: Ipv4Addr::new(10, 0, 2, 4),
                ..base
            },
            Lease {
                address: (base.address.0, 16),
                ..base
            },
            Lease {
                address: (Ipv4Addr::new(10, 0, 2, 16), 24),
                ..base
            },
            Lease {
                router: None,
                ..base
            },
            Lease {
                dns: [None; MAX_DNS],
                ..base
            },
        ];
        for other in differs {
            assert_eq!(run(&[base, other]), (2, "; the lease changed"), "{other:?}");
        }
    }

    #[test]
    fn the_first_lease_after_the_fallback_is_counted() {
        let kind = Kind::of(Was::Fallback, &lease());
        assert_eq!(
            (kind.counts(), kind.note()),
            (true, "; leaving the fallback")
        );
    }
}
