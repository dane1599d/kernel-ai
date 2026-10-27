//! Spike S05-1's calls (the `spikes` feature only; never merged).

use crate::process::futex;
use crate::spike_compose;
use kabi::{Error, call};

pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    Some(match number {
        call::SPIKE_DISPLAY_MAP => spike_compose::display_map(slot, a[0]),
        call::SPIKE_DISPLAY_FLUSH => spike_compose::display_flush(slot, a[0], a[1]),
        call::SPIKE_SHARE_MAP => spike_compose::share_map(slot, a[0], a[1]),
        call::SPIKE_BELL_WAIT => futex::wait_shared(slot, a[0], a[1], a[2]),
        call::SPIKE_BELL_RING => futex::wake_shared(slot, a[0], a[1]),
        call::SPIKE_MARK => {
            crate::log!("compose: ready for keys (a screendump)");
            Ok(0)
        }
        _ => return None,
    })
}
