//! The rules `desktop` follows to let go of a client's pools and of clients
//! whose process is gone, apart from the system calls, so they are host-tested.
//!
//! The kernel gives no call that says a pid is gone: its process list in the
//! metrics holds a fixed number of entries (16) and says how many it holds, so
//! a pid missing from a *full* list may be alive past the cut. A client is
//! therefore judged gone only on a list that is certainly whole (it has room to
//! spare) and that does not name it twice in a row, a second apart.

use alloc::vec::Vec;

/// Whether a pool is let go after a window of it closed: no window shows a
/// buffer of it, and the client holds none.
pub fn pool_unused(shown_in_it: bool, client_holds_one: bool) -> bool {
    !shown_in_it && !client_holds_one
}

/// Whether a metrics list of `entries` processes, in a table of `room`, is the
/// whole list: with room to spare, so nothing was cut off its end.
pub fn list_is_whole(entries: usize, room: usize) -> bool {
    entries < room
}

/// Which clients are gone, from the lists read a second or more apart.
#[derive(Debug, Default)]
pub struct Reaper {
    /// Clients the last whole list did not name.
    missing: Vec<u64>,
}

impl Reaper {
    /// The clients (`clients`, by pid) that are gone, given the pids the list
    /// names (`alive`) and whether it is whole ([`list_is_whole`]): those it
    /// has not named in two whole lists in a row. A list that may be cut says
    /// nothing, and forgets what the last said, so a client is never dropped on
    /// a pid that fell past the cut or that connected after the list was read.
    pub fn gone(&mut self, clients: &[u64], alive: &[u64], whole: bool) -> Vec<u64> {
        if !whole {
            self.missing.clear();
            return Vec::new();
        }
        let absent: Vec<u64> = clients
            .iter()
            .copied()
            .filter(|pid| !alive.contains(pid))
            .collect();
        let gone: Vec<u64> = absent
            .iter()
            .copied()
            .filter(|pid| self.missing.contains(pid))
            .collect();
        self.missing = absent.into_iter().filter(|p| !gone.contains(p)).collect();
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_goes_only_when_nothing_shows_or_holds_a_buffer_of_it() {
        assert!(pool_unused(false, false));
        assert!(!pool_unused(true, false), "a window still shows it");
        assert!(!pool_unused(false, true), "the client still holds a buffer");
        assert!(!pool_unused(true, true));
    }

    #[test]
    fn only_a_list_with_room_to_spare_is_whole() {
        assert!(list_is_whole(0, 16));
        assert!(list_is_whole(15, 16));
        assert!(!list_is_whole(16, 16), "it may be cut at its end");
    }

    #[test]
    fn a_client_is_gone_when_two_whole_lists_in_a_row_leave_it_out() {
        let mut reaper = Reaper::default();
        let clients = [10, 11, 12];
        assert!(reaper.gone(&clients, &[10, 11, 12], true).is_empty());
        // 11 is missing once: not yet.
        assert!(reaper.gone(&clients, &[10, 12], true).is_empty());
        // Missing again: gone.
        assert_eq!(reaper.gone(&clients, &[10, 12], true), [11]);
    }

    #[test]
    fn a_pid_back_in_the_list_or_past_the_cut_is_never_dropped() {
        let mut reaper = Reaper::default();
        let clients = [10, 11];
        assert!(reaper.gone(&clients, &[10], true).is_empty());
        // It reappears (a late snapshot): forgotten.
        assert!(reaper.gone(&clients, &[10, 11], true).is_empty());
        assert!(reaper.gone(&clients, &[10], true).is_empty());
        // A list that may be cut says nothing, and wipes what was suspected.
        assert!(reaper.gone(&clients, &[10], false).is_empty());
        assert!(reaper.gone(&clients, &[10], true).is_empty());
        // Sixteen processes listed, a client not among them: not gone.
        let sixteen: Vec<u64> = (100..116).collect();
        for _ in 0..5 {
            let whole = list_is_whole(sixteen.len(), 16);
            assert!(reaper.gone(&[10], &sixteen, whole).is_empty());
        }
    }

    #[test]
    fn a_client_that_connected_after_the_list_was_read_is_not_dropped_on_it() {
        let mut reaper = Reaper::default();
        // The list was read before pid 20 connected: absent once, then named.
        assert!(reaper.gone(&[10, 20], &[10], true).is_empty());
        assert!(reaper.gone(&[10, 20], &[10, 20], true).is_empty());
        assert!(reaper.gone(&[10, 20], &[10, 20], true).is_empty());
    }

    #[test]
    fn two_clients_gone_together_are_both_found() {
        let mut reaper = Reaper::default();
        let clients = [1, 2, 3];
        reaper.gone(&clients, &[3], true);
        assert_eq!(reaper.gone(&clients, &[3], true), [1, 2]);
        // They are dropped by the caller; the next list has no more to say.
        assert!(reaper.gone(&[3], &[3], true).is_empty());
    }
}
