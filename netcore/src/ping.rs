//! Round-trip statistics for `ping`.

use alloc::format;
use alloc::string::String;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u32,
    pub received: u32,
    min_us: u64,
    max_us: u64,
    total_us: u64,
}

impl Stats {
    pub fn sent(&mut self) {
        self.sent += 1;
    }

    /// A reply that took `rtt_us` microseconds.
    pub fn received(&mut self, rtt_us: u64) {
        self.min_us = if self.received == 0 {
            rtt_us
        } else {
            self.min_us.min(rtt_us)
        };
        self.max_us = self.max_us.max(rtt_us);
        self.total_us = self.total_us.saturating_add(rtt_us);
        self.received += 1;
    }

    /// `4 sent, 3 received, 25% lost; round trip min/avg/max 0.210/0.402/0.913 ms`.
    pub fn summary(&self) -> String {
        let lost = match self.sent {
            0 => 0,
            sent => (sent - self.received.min(sent)) * 100 / sent,
        };
        let mut text = format!(
            "{} sent, {} received, {lost}% lost",
            self.sent, self.received
        );
        if self.received > 0 {
            let average = self.total_us / u64::from(self.received);
            text.push_str(&format!(
                "; round trip min/avg/max {}/{}/{} ms",
                millis(self.min_us),
                millis(average),
                millis(self.max_us)
            ));
        }
        text
    }
}

/// Microseconds as milliseconds with three decimals.
pub fn millis(us: u64) -> String {
    format!("{}.{:03}", us / 1000, us % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_losses_and_round_trips() {
        let mut stats = Stats::default();
        assert_eq!(stats.summary(), "0 sent, 0 received, 0% lost");
        for rtt in [Some(913), None, Some(210), Some(83)] {
            stats.sent();
            if let Some(rtt) = rtt {
                stats.received(rtt);
            }
        }
        assert_eq!(
            stats.summary(),
            "4 sent, 3 received, 25% lost; round trip min/avg/max 0.083/0.402/0.913 ms"
        );
    }

    #[test]
    fn millis_keeps_three_decimals() {
        assert_eq!(millis(0), "0.000");
        assert_eq!(millis(1_234_567), "1234.567");
    }
}
