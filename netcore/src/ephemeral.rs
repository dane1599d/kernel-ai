//! Ephemeral ports for the kernel's connects and binds (RFC 6335's 49152 to
//! 65535): one counter, moved on by one a port, that comes round every
//! [`COUNT`]. A port a socket still holds is skipped (bug hunt 5's H5-19): a
//! new connect on a live stream's port to the same server would have that
//! stream's four-tuple, and the server's segments for it could reach the new
//! socket, which answers them with a reset.

/// The first ephemeral port.
pub const FIRST: u16 = 49152;
/// Ephemeral ports in all: [`FIRST`] to 65535.
pub const COUNT: u16 = 16384;

/// The port counter value `n` stands for: `n` folded into the range, so the
/// counter's own wrap at 65536, a multiple of [`COUNT`], leaves no gap.
pub fn fold(n: u16) -> u16 {
    FIRST + n.wrapping_sub(FIRST) % COUNT
}

/// The first port the counter gives that `held` does not name: `next()`
/// returns the counter and moves it on by one, once for each port looked
/// at. After a whole turn of [`COUNT`] ports, every one held, None.
pub fn next_free(mut next: impl FnMut() -> u16, held: impl Fn(u16) -> bool) -> Option<u16> {
    core::iter::repeat_with(|| fold(next()))
        .take(usize::from(COUNT))
        .find(|&port| !held(port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A counter from `start`, moved on as the kernel's `NEXT_PORT` is.
    fn counter(start: u16) -> impl FnMut() -> u16 {
        let mut n = start;
        move || {
            let drawn = n;
            n = n.wrapping_add(1);
            drawn
        }
    }

    #[test]
    fn the_counter_folds_into_the_range_and_comes_round_every_16384() {
        assert_eq!(fold(FIRST), FIRST);
        assert_eq!(fold(FIRST + 5), FIRST + 5);
        assert_eq!(fold(u16::MAX), u16::MAX);
        assert_eq!(fold(0), FIRST, "the counter's own wrap");
        assert_eq!(fold(5), FIRST + 5);
        // What H5-19 found: 16384 draws on, the same port again.
        let mut next = counter(60_000);
        let first = fold(next());
        for _ in 1..COUNT {
            assert_ne!(fold(next()), first);
        }
        assert_eq!(fold(next()), first);
    }

    #[test]
    fn a_port_a_socket_holds_is_skipped_when_the_counter_comes_round_to_it() {
        let held = [60_000, 60_001];
        let mut draws = 0;
        let mut next = counter(60_000);
        let port = next_free(
            || {
                draws += 1;
                next()
            },
            |port| held.contains(&port),
        );
        assert_eq!(port, Some(60_002));
        assert_eq!(draws, 3, "one draw for each port looked at");
        // A free port is taken at once, as before.
        let free = next_free(counter(50_000), |port| held.contains(&port));
        assert_eq!(free, Some(50_000));
    }

    #[test]
    fn the_skip_goes_past_the_top_of_the_range_to_the_first_port() {
        let port = next_free(counter(u16::MAX), |port| port == u16::MAX);
        assert_eq!(port, Some(FIRST));
    }

    #[test]
    fn with_every_port_held_it_is_none_after_one_whole_turn() {
        let mut draws = 0u32;
        let mut next = counter(FIRST);
        let port = next_free(
            || {
                draws += 1;
                next()
            },
            |_| true,
        );
        assert_eq!(port, None);
        assert_eq!(draws, u32::from(COUNT));
    }
}
