//! In-memory backoff for failed password checks, per client network and per account.
//!
//! Every attempt counts as a failure until it is reported as a success, so
//! concurrent guesses cannot all slip past the check. After
//! [`FREE_ATTEMPTS`] failures a key is blocked for one second, doubling with
//! each further failure up to [`MAX_DELAY`]. Keys forget their failures after
//! [`FORGET_AFTER`] without attempts.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    sync::Mutex,
    time::Duration,
};

use tokio::time::Instant;

/// Failures allowed before a key is blocked.
const FREE_ATTEMPTS: u32 = 5;
/// Block after the first failure beyond the free ones.
const FIRST_DELAY: Duration = Duration::from_secs(1);
/// Longest block.
const MAX_DELAY: Duration = Duration::from_secs(15 * 60);
/// Idle time after which a key's failures are forgotten.
const FORGET_AFTER: Duration = Duration::from_secs(60 * 60);
/// Tracked keys; when full after dropping forgotten keys, new keys go untracked.
const MAX_KEYS: usize = 65_536;

/// Who an attempt is counted against.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Key {
    /// An IPv4 address or an IPv6 /64 network, which one client usually controls whole.
    Client(IpAddr),
    /// A user name in upper case, or an account id.
    Account(String),
}

#[derive(Debug)]
struct Entry {
    failures: u32,
    last_attempt: Instant,
    blocked_until: Option<Instant>,
}

/// Shared failure counters; cheap to keep in long-lived state.
#[derive(Debug, Default)]
pub(crate) struct LoginThrottle {
    entries: Mutex<HashMap<Key, Entry>>,
}

/// An admitted attempt; report a correct password with [`Attempt::succeeded`]. Dropping it
/// otherwise confirms the failure and restarts the key's block from that moment, so the time
/// spent checking the password never shortens it.
#[must_use = "an attempt stays counted as a failure unless it succeeded"]
#[derive(Debug)]
pub(crate) struct Attempt<'a> {
    throttle: &'a LoginThrottle,
    keys: Vec<Key>,
    succeeded: bool,
}

impl LoginThrottle {
    /// Admits an attempt from `client` for `account` (a user name or account id),
    /// counting it as a failure, or returns how long the caller must wait.
    pub(crate) fn begin(
        &self,
        client: Option<IpAddr>,
        account: &str,
    ) -> Result<Attempt<'_>, Duration> {
        let keys: Vec<Key> = client
            .map(|address| Key::Client(network(address)))
            .into_iter()
            .chain(std::iter::once(Key::Account(account.trim().to_uppercase())))
            .collect();
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("login throttle lock poisoned");
        let wait = keys
            .iter()
            .filter_map(|key| entries.get(key))
            .filter(|entry| now.duration_since(entry.last_attempt) < FORGET_AFTER)
            .filter_map(|entry| entry.blocked_until)
            .filter(|until| *until > now)
            .max();
        if let Some(until) = wait {
            return Err(until - now);
        }
        if entries.len() >= MAX_KEYS {
            entries.retain(|_, entry| now.duration_since(entry.last_attempt) < FORGET_AFTER);
        }
        for key in &keys {
            if !entries.contains_key(key) && entries.len() >= MAX_KEYS {
                continue;
            }
            let entry = entries.entry(key.clone()).or_insert(Entry {
                failures: 0,
                last_attempt: now,
                blocked_until: None,
            });
            if now.duration_since(entry.last_attempt) >= FORGET_AFTER {
                entry.failures = 0;
            }
            entry.failures = entry.failures.saturating_add(1);
            entry.last_attempt = now;
            entry.blocked_until = delay(entry.failures).map(|delay| now + delay);
        }
        Ok(Attempt {
            throttle: self,
            keys,
            succeeded: false,
        })
    }
}

impl Attempt<'_> {
    /// Clears the failures of every key of the attempt.
    pub(crate) fn succeeded(mut self) {
        self.succeeded = true;
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        // A poisoned map only loses this update; panicking in a destructor could abort.
        let Ok(mut entries) = self.throttle.entries.lock() else {
            return;
        };
        let now = Instant::now();
        for key in &self.keys {
            if self.succeeded {
                entries.remove(key);
            } else if let Some(entry) = entries.get_mut(key) {
                entry.blocked_until = delay(entry.failures).map(|delay| now + delay);
            }
        }
    }
}

/// Block after `failures` consecutive failures.
fn delay(failures: u32) -> Option<Duration> {
    let doublings = failures.checked_sub(FREE_ATTEMPTS)?;
    Some(
        2_u32
            .checked_pow(doublings)
            .and_then(|factor| FIRST_DELAY.checked_mul(factor))
            .map_or(MAX_DELAY, |delay| delay.min(MAX_DELAY)),
    )
}

fn network(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(
            || {
                let [a, b, c, d, ..] = v6.segments();
                IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
            },
            IpAddr::V4,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 7)));

    fn fail(throttle: &LoginThrottle, client: Option<IpAddr>, account: &str) {
        drop(throttle.begin(client, account).expect("admitted"));
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_failures_block_with_growing_delays_until_they_expire() {
        let throttle = LoginThrottle::default();
        for _ in 0..FREE_ATTEMPTS - 1 {
            fail(&throttle, CLIENT, "admin");
        }
        fail(&throttle, CLIENT, "admin");
        assert_eq!(throttle.begin(CLIENT, "admin").err(), Some(FIRST_DELAY));
        tokio::time::advance(FIRST_DELAY).await;
        fail(&throttle, CLIENT, "admin");
        assert_eq!(throttle.begin(CLIENT, "admin").err(), Some(FIRST_DELAY * 2));
        tokio::time::advance(FORGET_AFTER).await;
        fail(&throttle, CLIENT, "admin");
        assert!(throttle.begin(CLIENT, "admin").is_ok(), "failures were forgotten");
    }

    #[tokio::test(start_paused = true)]
    async fn a_block_starts_when_the_failed_check_ends() {
        let throttle = LoginThrottle::default();
        for _ in 0..FREE_ATTEMPTS - 1 {
            fail(&throttle, CLIENT, "admin");
        }
        let slow_check = throttle.begin(CLIENT, "admin").expect("admitted");
        tokio::time::advance(FIRST_DELAY / 2).await;
        drop(slow_check);
        assert_eq!(throttle.begin(CLIENT, "admin").err(), Some(FIRST_DELAY));
    }

    #[tokio::test(start_paused = true)]
    async fn names_and_client_networks_are_blocked_independently() {
        let throttle = LoginThrottle::default();
        for _ in 0..FREE_ATTEMPTS {
            fail(&throttle, CLIENT, "admin");
        }
        let other_client = Some("198.51.100.1".parse().expect("address"));
        assert!(throttle.begin(other_client, "ADMIN").is_err(), "name is blocked");
        assert!(throttle.begin(CLIENT, "other").is_err(), "client is blocked");

        for host in 1..=FREE_ATTEMPTS {
            let host = u16::try_from(host).expect("small");
            let client = Some(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, host)));
            fail(&throttle, client, &format!("name{host}"));
        }
        let same_network = Some("2001:db8:0:1::ffff".parse().expect("address"));
        assert!(throttle.begin(same_network, "fresh").is_err(), "IPv6 /64 is one client");
    }

    #[tokio::test(start_paused = true)]
    async fn success_clears_the_failures() {
        let throttle = LoginThrottle::default();
        for _ in 0..FREE_ATTEMPTS - 1 {
            fail(&throttle, CLIENT, "admin");
        }
        throttle.begin(CLIENT, "admin").expect("admitted").succeeded();
        for _ in 0..FREE_ATTEMPTS - 1 {
            fail(&throttle, CLIENT, "admin");
        }
        assert!(throttle.begin(CLIENT, "admin").is_ok());
    }

    #[test]
    fn delays_double_up_to_the_maximum() {
        assert_eq!(delay(FREE_ATTEMPTS - 1), None);
        assert_eq!(delay(FREE_ATTEMPTS), Some(FIRST_DELAY));
        assert_eq!(delay(FREE_ATTEMPTS + 3), Some(FIRST_DELAY * 8));
        assert_eq!(delay(FREE_ATTEMPTS + 40), Some(MAX_DELAY));
        assert_eq!(delay(u32::MAX), Some(MAX_DELAY));
    }
}
