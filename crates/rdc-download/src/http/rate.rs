//! Speed limiting and speed measurement.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Bytes worth of this much time at the configured rate may be received in one burst.
const BURST: Duration = Duration::from_millis(250);
/// Speed is averaged over this much recent history.
const SPEED_WINDOW: Duration = Duration::from_secs(2);

/// A token bucket shared by all connections of one download.
#[derive(Debug)]
pub(crate) struct Throttle {
    /// The instant at which every byte granted so far has been paid for at the current rate.
    paid_until: Mutex<Instant>,
}

impl Throttle {
    pub(crate) fn new() -> Self {
        Self {
            paid_until: Mutex::new(Instant::now()),
        }
    }

    /// Accounts for `bytes` received under a limit of `rate` bytes per second (`0` means
    /// unlimited) and returns how long the caller must wait before reading more.
    pub(crate) fn reserve(&self, bytes: u64, rate: u64) -> Duration {
        let now = Instant::now();
        let mut paid_until = self.paid_until.lock().expect("throttle lock poisoned");
        if rate == 0 {
            *paid_until = now;
            return Duration::ZERO;
        }
        let earliest = now.checked_sub(BURST).unwrap_or(now);
        let start = (*paid_until).max(earliest);
        let cost_nanos = u128::from(bytes) * 1_000_000_000 / u128::from(rate);
        let cost = Duration::from_nanos(u64::try_from(cost_nanos).unwrap_or(u64::MAX));
        *paid_until = start + cost;
        paid_until.saturating_duration_since(now)
    }
}

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
    fn unlimited_rate_never_waits() {
        let throttle = Throttle::new();
        assert_eq!(throttle.reserve(10_000_000, 0), Duration::ZERO);
    }

    #[test]
    fn limited_rate_waits_for_bytes_beyond_the_rate() {
        let throttle = Throttle::new();
        let wait = throttle.reserve(1000, 1000);
        assert!(wait > Duration::from_millis(900), "wait {wait:?}");
        let wait = throttle.reserve(500, 1000);
        assert!(wait > Duration::from_millis(1400), "wait {wait:?}");
    }

    #[test]
    fn idle_time_allows_a_short_burst_only() {
        let throttle = Throttle::new();
        std::thread::sleep(Duration::from_millis(300));
        // 250 ms of credit at 1000 B/s covers 250 bytes.
        assert!(throttle.reserve(200, 1000) < Duration::from_millis(5));
        let wait = throttle.reserve(1000, 1000);
        assert!(wait > Duration::from_millis(900), "wait {wait:?}");
    }

    #[test]
    fn switching_to_unlimited_clears_the_debt() {
        let throttle = Throttle::new();
        throttle.reserve(10_000, 1000);
        assert_eq!(throttle.reserve(1, 0), Duration::ZERO);
        assert!(throttle.reserve(1, 1000) < Duration::from_millis(5));
    }

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
