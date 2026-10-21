//! The resolver's cache: answers kept for their TTL, at most [`CAPACITY`] names.

use crate::dns::Answer;
use alloc::string::String;
use alloc::vec::Vec;
use core::net::Ipv4Addr;

pub const CAPACITY: usize = 32;
/// The longest an answer is kept, whatever its TTL says.
pub const MAX_TTL_S: u32 = 86_400;

struct Entry {
    host: String,
    addresses: Vec<Ipv4Addr>,
    expires_ms: u64,
}

pub struct Cache {
    entries: Vec<Entry>,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The addresses kept for `host` (normalized) and the seconds they have
    /// left, rounded up, unless they have expired by `now_ms`.
    pub fn get(&self, host: &str, now_ms: u64) -> Option<(&[Ipv4Addr], u64)> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.host == host && entry.expires_ms > now_ms)?;
        Some((&entry.addresses, (entry.expires_ms - now_ms).div_ceil(1000)))
    }

    /// Keeps `answer` for `host` until its TTL runs out. A TTL of 0 means the
    /// answer must not be kept; a longer one than [`MAX_TTL_S`] is cut to it.
    /// When full, an expired entry makes room, or else the one closest to expiring.
    pub fn put(&mut self, host: &str, answer: &Answer, now_ms: u64) {
        self.entries.retain(|entry| entry.host != host);
        if answer.ttl == 0 || answer.addresses.is_empty() {
            return;
        }
        if self.entries.len() >= CAPACITY {
            let victim = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.expires_ms)
                .map(|(index, _)| index);
            if let Some(index) = victim {
                self.entries.swap_remove(index);
            }
        }
        self.entries.push(Entry {
            host: String::from(host),
            addresses: answer.addresses.clone(),
            expires_ms: now_ms.saturating_add(u64::from(answer.ttl.min(MAX_TTL_S)) * 1000),
        });
    }

    /// Entries not yet expired at `now_ms`.
    pub fn live(&self, now_ms: u64) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.expires_ms > now_ms)
            .count()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    fn answer(last: u8, ttl: u32) -> Answer {
        Answer {
            addresses: vec![Ipv4Addr::new(10, 0, 2, last)],
            ttl,
        }
    }

    #[test]
    fn an_answer_lasts_exactly_its_ttl() {
        let mut cache = Cache::new();
        cache.put("kai.test", &answer(99, 2), 1_000);
        assert_eq!(
            cache.get("kai.test", 1_000).map(|(a, s)| (a[0], s)),
            Some((Ipv4Addr::new(10, 0, 2, 99), 2))
        );
        assert_eq!(cache.get("kai.test", 2_500).map(|(_, s)| s), Some(1));
        assert!(cache.get("kai.test", 2_999).is_some());
        assert!(cache.get("kai.test", 3_000).is_none());
        assert!(cache.get("other.test", 1_000).is_none());
        assert_eq!(cache.live(1_000), 1);
        assert_eq!(cache.live(3_000), 0);
    }

    #[test]
    fn ttl_zero_is_not_kept_and_replaces_an_older_answer() {
        let mut cache = Cache::new();
        cache.put("kai.test", &answer(99, 60), 0);
        cache.put("kai.test", &answer(98, 0), 0);
        assert!(cache.get("kai.test", 0).is_none());
    }

    #[test]
    fn a_huge_ttl_is_capped_at_a_day() {
        let mut cache = Cache::new();
        cache.put("kai.test", &answer(99, u32::MAX), 0);
        assert_eq!(
            cache.get("kai.test", 0).map(|(_, s)| s),
            Some(u64::from(MAX_TTL_S))
        );
    }

    #[test]
    fn a_full_cache_drops_the_entry_closest_to_expiring() {
        let mut cache = Cache::new();
        for i in 0..CAPACITY {
            cache.put(&format!("h{i}.test"), &answer(1, 100 + i as u32), 0);
        }
        cache.put("new.test", &answer(2, 50), 0);
        assert!(cache.get("h0.test", 0).is_none());
        assert!(cache.get("h1.test", 0).is_some());
        assert!(cache.get("new.test", 0).is_some());
        assert_eq!(cache.live(0), CAPACITY);
        cache.clear();
        assert_eq!(cache.live(0), 0);
    }
}
