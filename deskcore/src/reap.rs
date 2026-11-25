//! The rules `desktop` follows to let go of a client's pools and of clients
//! whose process is gone, apart from the system calls, so they are host-tested.
//!
//! The kernel says whether a pid still runs (`PROC_ALIVE`): a client is gone
//! only when it answers no. An answer that is not a yes or a no (the call
//! failed) says nothing, and the client stays.

use alloc::vec::Vec;

/// Whether a pool is let go after a window of it closed: no window shows a
/// buffer of it, and the client holds none.
pub fn pool_unused(shown_in_it: bool, client_holds_one: bool) -> bool {
    !shown_in_it && !client_holds_one
}

/// The clients (by pid) whose process is gone: those `alive` says are not
/// running (`Some(false)`). A client it cannot say anything of (`None`) stays.
pub fn gone(clients: &[u64], mut alive: impl FnMut(u64) -> Option<bool>) -> Vec<u64> {
    clients
        .iter()
        .copied()
        .filter(|&pid| alive(pid) == Some(false))
        .collect()
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
    fn a_client_is_gone_when_the_kernel_says_its_process_is_not_running() {
        let running = [10, 12];
        let said = |pid: u64| Some(running.contains(&pid));
        assert_eq!(gone(&[10, 11, 12, 13], said), [11, 13]);
        assert!(gone(&[10, 12], said).is_empty());
        assert!(gone(&[], said).is_empty());
    }

    #[test]
    fn a_client_the_kernel_cannot_say_anything_of_stays() {
        // The call failed for 11: it is not dropped on a guess; 12 is gone.
        let said = |pid: u64| match pid {
            11 => None,
            12 => Some(false),
            _ => Some(true),
        };
        assert_eq!(gone(&[10, 11, 12], said), [12]);
    }

    #[test]
    fn sixteen_clients_are_each_asked_about_once_with_no_list_to_cut() {
        // There is no list to cut at 16 any more: sixteen clients, one gone.
        let mut asked = Vec::new();
        let clients: Vec<u64> = (100..116).collect();
        let found = gone(&clients, |pid| {
            asked.push(pid);
            Some(pid != 107)
        });
        assert_eq!(found, [107]);
        assert_eq!(asked, clients);
    }
}
