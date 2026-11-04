//! Speed measurement.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Speed is averaged over this much recent history.
const SPEED_WINDOW: Duration = Duration::from_secs(2);

/// Averages the transfer speed over a short sliding window.
#[derive(Debug)]
pub(crate) struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    /// Records the total byte count at `now` and returns the speed in bytes per second.
    pub(crate) fn record(&mut self, now: Instant, total: u64) -> u64 {
        self.samples.push_back((now, total));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
        let Some(&(first_at, first_total)) = self.samples.front() else {
            return 0;
        };
        let elapsed = now.saturating_duration_since(first_at).as_millis();
        if elapsed == 0 {
            return 0;
        }
        let bytes = u128::from(total.saturating_sub(first_total));
        u64::try_from(bytes * 1000 / elapsed).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_is_averaged_over_the_window() {
        let mut meter = SpeedMeter::new();
        let start = Instant::now();
        assert_eq!(meter.record(start, 0), 0);
        assert_eq!(meter.record(start + Duration::from_millis(500), 500), 1000);
        assert_eq!(meter.record(start + Duration::from_secs(1), 1000), 1000);
    }

    #[test]
    fn old_samples_leave_the_window() {
        let mut meter = SpeedMeter::new();
        let start = Instant::now();
        meter.record(start, 0);
        meter.record(start + Duration::from_secs(1), 10_000);
        meter.record(start + Duration::from_secs(3), 10_000);
        // Only the last two seconds count: no progress there.
        assert_eq!(meter.record(start + Duration::from_secs(4), 10_000), 0);
    }
}
