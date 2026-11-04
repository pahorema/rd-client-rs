//! Process-wide bookkeeping of rate-limit cooldowns.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};

/// Cooldown key used when the whole provider (not a single host) has to pause.
pub const GENERAL_KEY: &str = "General";

/// Remembers until when requests to a key (usually a host name) must not be sent.
///
/// Cooldowns only ever grow: a shorter cooldown never shortens an existing one.
/// Expired entries are dropped lazily by the getters.
#[derive(Debug, Default)]
pub struct RateLimitCoordinator {
    cooldowns: Mutex<HashMap<String, Timestamp>>,
}

impl RateLimitCoordinator {
    /// Creates a coordinator without any cooldown.
    pub fn new() -> Self {
        Self::default()
    }

    /// Blocks `key` for `retry_after` from now, keeping a later existing cooldown.
    pub fn update_cooldown(&self, key: &str, retry_after: Duration) {
        let next_allowed_at = Timestamp::now()
            .checked_add(to_signed(retry_after))
            .unwrap_or(Timestamp::MAX);
        let mut cooldowns = self.lock();
        cooldowns
            .entry(key.to_owned())
            .and_modify(|existing| {
                if next_allowed_at > *existing {
                    *existing = next_allowed_at;
                }
            })
            .or_insert(next_allowed_at);
    }

    /// Time left before `key` may be used again, zero when it is not blocked.
    pub fn remaining_cooldown(&self, key: &str) -> Duration {
        self.next_allowed_at(key).map_or(Duration::ZERO, |at| {
            to_unsigned(at.duration_since(Timestamp::now()))
        })
    }

    /// Instant from which `key` may be used again, `None` when it is not blocked.
    pub fn next_allowed_at(&self, key: &str) -> Option<Timestamp> {
        let now = Timestamp::now();
        let mut cooldowns = self.lock();
        match cooldowns.get(key) {
            Some(at) if *at > now => Some(*at),
            Some(_) => {
                cooldowns.remove(key);
                None
            }
            None => None,
        }
    }

    /// The longest time left across all keys, zero when nothing is blocked.
    pub fn max_remaining_cooldown(&self) -> Duration {
        self.max_next_allowed_at().map_or(Duration::ZERO, |at| {
            to_unsigned(at.duration_since(Timestamp::now()))
        })
    }

    /// The latest instant any key is blocked until, `None` when nothing is blocked.
    pub fn max_next_allowed_at(&self) -> Option<Timestamp> {
        let now = Timestamp::now();
        let mut cooldowns = self.lock();
        cooldowns.retain(|_, at| *at > now);
        cooldowns.values().max().copied()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Timestamp>> {
        self.cooldowns.lock().expect("rate-limit lock poisoned")
    }
}

fn to_signed(duration: Duration) -> SignedDuration {
    SignedDuration::try_from(duration).unwrap_or(SignedDuration::MAX)
}

fn to_unsigned(duration: SignedDuration) -> Duration {
    Duration::try_from(duration).unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn update_cooldown_blocks_the_key() {
        let coordinator = RateLimitCoordinator::new();

        coordinator.update_cooldown("api.example.com", 5 * MINUTE);

        let remaining = coordinator.remaining_cooldown("api.example.com");
        assert!(
            remaining > 4 * MINUTE && remaining <= 5 * MINUTE,
            "{remaining:?}"
        );
    }

    #[test]
    fn remaining_cooldown_is_zero_for_unknown_keys() {
        let coordinator = RateLimitCoordinator::new();

        assert_eq!(
            coordinator.remaining_cooldown("api.example.com"),
            Duration::ZERO
        );
        assert_eq!(coordinator.next_allowed_at("api.example.com"), None);
    }

    #[test]
    fn shorter_cooldowns_do_not_shorten_existing_ones() {
        let coordinator = RateLimitCoordinator::new();

        coordinator.update_cooldown("host", 10 * MINUTE);
        coordinator.update_cooldown("host", MINUTE);

        assert!(coordinator.remaining_cooldown("host") > 9 * MINUTE);
    }

    #[test]
    fn max_next_allowed_at_returns_the_latest_cooldown() {
        let coordinator = RateLimitCoordinator::new();

        coordinator.update_cooldown("a", 10 * MINUTE);
        coordinator.update_cooldown("b", 20 * MINUTE);

        let max = coordinator
            .max_next_allowed_at()
            .expect("a cooldown is active");
        let expected = Timestamp::now() + SignedDuration::from_mins(20);
        let diff = expected.duration_since(max).as_secs().abs();
        assert!(diff <= 1, "max was {max}, expected about {expected}");
        assert!(coordinator.max_remaining_cooldown() > 19 * MINUTE);
    }

    #[test]
    fn max_next_allowed_at_is_none_when_all_cooldowns_expired() {
        let coordinator = RateLimitCoordinator::new();
        coordinator.lock().insert(
            "old".to_owned(),
            Timestamp::now() - SignedDuration::from_secs(10),
        );

        assert_eq!(coordinator.max_next_allowed_at(), None);
        assert_eq!(coordinator.max_remaining_cooldown(), Duration::ZERO);
    }
}
