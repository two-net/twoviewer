//! Limits per network, kept in memory: how often one network may sign up (AUTH-15), and how often
//! it may fail to sign in (SEC-08); and how often sign-ups may email one address.
//!
//! They are counted by each instance of the server on its own. With several instances behind a
//! load balancer (CAP-03), a network can do that many times more before it is challenged or
//! refused. Nothing about them is stored, so no IP addresses are kept beyond the audit events
//! (PRIV-02). The limit on failed sign-ins per address (AUTH-08) must hold across instances, so it
//! is in the database instead ([`crate::throttle`]).

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use crate::{config, db::EmailAddress};

/// What a sign-up from a network may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Go ahead.
    Free,
    /// Go ahead with a solved challenge.
    Challenge,
    /// Not now: the network has reached `sign_up.max_per_network`. A new sign-up is possible after
    /// `retry_after`.
    Refused { retry_after: Duration },
}

/// A network, as sign-ups and failed sign-ins are counted: an IPv4 address, or the first 64 bits
/// of an IPv6 address, since one connection to the internet usually gets a whole /64.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Network {
    V4(u32),
    V6(u64),
}

impl From<IpAddr> for Network {
    fn from(address: IpAddr) -> Self {
        match address.to_canonical() {
            IpAddr::V4(address) => Self::V4(address.to_bits()),
            IpAddr::V6(address) => Self::V6((address.to_bits() >> 64) as u64),
        }
    }
}

pub struct Limits {
    sign_up: config::SignUp,
    sign_in: config::SignIn,
    /// Sign-ups per network.
    sign_ups: Windows,
    /// Failed sign-ins per network.
    sign_in_failures: Windows,
    /// When sign-up last emailed each address, by the SHA-256 of the address, so that the
    /// addresses themselves aren't kept.
    notices: Mutex<Counts<[u8; 32], Instant>>,
}

/// A map whose expired entries are removed at most once per `interval`.
struct Counts<K, V> {
    entries: HashMap<K, V>,
    last_pruned: Option<Instant>,
}

impl<K: Eq + std::hash::Hash, V> Counts<K, V> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            last_pruned: None,
        }
    }

    /// Removes the entries that `expired` says are, unless that was done less than `interval`
    /// ago.
    fn prune(&mut self, now: Instant, interval: Duration, expired: impl Fn(&V) -> bool) {
        if self
            .last_pruned
            .is_some_and(|last| now.saturating_duration_since(last) < interval)
        {
            return;
        }
        self.entries.retain(|_, value| !expired(value));
        self.last_pruned = Some(now);
    }
}

/// How often a full table of networks is searched for room. Searching takes time proportional to
/// its size, so it isn't done for every request.
const FULL_PRUNE_INTERVAL: Duration = Duration::from_secs(1);

/// The times of events per network within a sliding `window`, such as sign-ups, for at most
/// `max_tracked` networks at once.
struct Windows {
    window: Duration,
    max_tracked: usize,
    networks: Mutex<Counts<Network, VecDeque<Instant>>>,
}

impl Windows {
    fn new(window: Duration, max_tracked: usize) -> Self {
        Self {
            window,
            max_tracked,
            networks: Mutex::new(Counts::new()),
        }
    }

    /// Runs `f` on the events of `network` within the window at `now`, oldest first, which it may
    /// add to. Returns `None` without running it when the table is full and `network` isn't in it,
    /// even after the networks without recent events have been removed.
    fn update<R>(
        &self,
        network: Network,
        now: Instant,
        f: impl FnOnce(&mut VecDeque<Instant>) -> R,
    ) -> Option<R> {
        let window = self.window;
        let expired = |events: &VecDeque<Instant>| {
            events
                .back()
                .is_none_or(|last| now.saturating_duration_since(*last) >= window)
        };
        let mut networks = self.networks.lock().expect("never poisoned");
        networks.prune(now, window, expired);
        if !networks.entries.contains_key(&network) && networks.entries.len() >= self.max_tracked {
            networks.prune(now, FULL_PRUNE_INTERVAL, expired);
            if networks.entries.len() >= self.max_tracked {
                return None;
            }
        }
        let events = networks.entries.entry(network).or_default();
        while events
            .front()
            .is_some_and(|first| now.saturating_duration_since(*first) >= window)
        {
            events.pop_front();
        }
        Some(f(events))
    }

    /// How many events `network` had within the window at `now`, and how long until the oldest of
    /// them leaves it. It changes nothing.
    fn count(&self, network: Network, now: Instant) -> (usize, Duration) {
        let networks = self.networks.lock().expect("never poisoned");
        let Some(events) = networks.entries.get(&network) else {
            return (0, Duration::ZERO);
        };
        let mut recent = events
            .iter()
            .map(|event| now.saturating_duration_since(*event))
            .filter(|age| *age < self.window);
        let Some(oldest) = recent.next() else {
            return (0, Duration::ZERO);
        };
        (1 + recent.count(), self.window.saturating_sub(oldest))
    }

    /// How long until the oldest of `events` leaves the window.
    fn retry_after(&self, events: &VecDeque<Instant>, now: Instant) -> Duration {
        events.front().map_or(Duration::ZERO, |first| {
            self.window
                .saturating_sub(now.saturating_duration_since(*first))
        })
    }
}

impl Limits {
    pub fn new(sign_up: &config::SignUp, sign_in: &config::SignIn) -> Self {
        Self {
            sign_up: sign_up.clone(),
            sign_in: sign_in.clone(),
            sign_ups: Windows::new(sign_up.window, sign_up.max_tracked_networks),
            sign_in_failures: Windows::new(sign_in.failure_window, sign_in.max_tracked_networks),
            notices: Mutex::new(Counts::new()),
        }
    }

    /// Counts a sign-up from `address` at `now`, and says what it may do.
    pub fn admit(&self, address: IpAddr, now: Instant) -> Admission {
        let config = &self.sign_up;
        self.sign_ups
            .update(Network::from(address), now, |attempts| {
                if attempts.len() >= config.max_per_network {
                    return Admission::Refused {
                        retry_after: self.sign_ups.retry_after(attempts, now),
                    };
                }
                attempts.push_back(now);
                if attempts.len() > config.challenge_after {
                    Admission::Challenge
                } else {
                    Admission::Free
                }
            })
            // Too many networks are signing up to count another one, which is suspicious in
            // itself.
            .unwrap_or(Admission::Challenge)
    }

    /// Whether sign-in from `address` is refused at `now`, because its network has failed to sign
    /// in `sign_in.max_failures_per_network` times within `sign_in.failure_window`. If it is, a
    /// new attempt is possible after the returned time.
    pub fn sign_in_refused(&self, address: IpAddr, now: Instant) -> Option<Duration> {
        let (failures, retry_after) = self.sign_in_failures.count(Network::from(address), now);
        (failures >= self.sign_in.max_failures_per_network).then_some(retry_after)
    }

    /// Counts a failed sign-in from `address` at `now`. While too many networks are counted, it
    /// isn't: the limit per address ([`crate::throttle`]) still holds.
    pub fn sign_in_failed(&self, address: IpAddr, now: Instant) {
        self.sign_in_failures
            .update(Network::from(address), now, |failures| {
                failures.push_back(now)
            });
    }

    /// Whether a sign-up at `now` may email `address`, at most once per `sign_up.notice_interval`.
    /// If it may, the time is recorded.
    ///
    /// The table needs no limit of its own: each entry follows a password hash, which limits how
    /// fast they come (`passwords.max_concurrent_hashes`).
    pub fn may_notify(&self, address: &EmailAddress, now: Instant) -> bool {
        let interval = self.sign_up.notice_interval;
        let key: [u8; 32] = Sha256::digest(address.as_str().as_bytes()).into();
        let mut notices = self.notices.lock().expect("never poisoned");
        notices.prune(now, interval, |last| {
            now.saturating_duration_since(*last) >= interval
        });
        match notices.entries.get(&key) {
            Some(last) if now.saturating_duration_since(*last) < interval => false,
            _ => {
                notices.entries.insert(key, now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(configure: impl FnOnce(&mut config::SignUp)) -> Limits {
        let mut config = config::SignUp {
            window: Duration::from_secs(60),
            challenge_after: 2,
            max_per_network: 4,
            max_tracked_networks: 100,
            ..config::SignUp::default()
        };
        configure(&mut config);
        Limits::new(&config, &config::SignIn::default())
    }

    fn ip(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    #[test]
    fn a_network_is_challenged_then_refused_within_the_window() {
        let limits = limits(|_| {});
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let address = ip("203.0.113.7");

        assert_eq!(limits.admit(address, at(0)), Admission::Free);
        assert_eq!(limits.admit(address, at(10)), Admission::Free);
        assert_eq!(limits.admit(address, at(20)), Admission::Challenge);
        assert_eq!(limits.admit(address, at(30)), Admission::Challenge);
        assert_eq!(
            limits.admit(address, at(40)),
            Admission::Refused {
                retry_after: Duration::from_secs(20)
            }
        );
        // Other networks are counted on their own.
        assert_eq!(limits.admit(ip("203.0.113.8"), at(40)), Admission::Free);
        // The first sign-up has left the window, so there is room for one more.
        assert_eq!(limits.admit(address, at(60)), Admission::Challenge);
        // Once the window has passed, the network starts afresh.
        assert_eq!(limits.admit(address, at(200)), Admission::Free);
    }

    #[test]
    fn challenge_after_zero_challenges_every_sign_up() {
        let limits = limits(|config| config.challenge_after = 0);

        assert_eq!(
            limits.admit(ip("203.0.113.7"), Instant::now()),
            Admission::Challenge
        );
    }

    #[test]
    fn ipv6_addresses_count_by_slash_64() {
        let limits = limits(|config| config.challenge_after = 1);
        let now = Instant::now();

        assert_eq!(limits.admit(ip("2001:db8:1:2::1"), now), Admission::Free);
        assert_eq!(
            limits.admit(ip("2001:db8:1:2:ffff::9"), now),
            Admission::Challenge
        );
        assert_eq!(limits.admit(ip("2001:db8:1:3::1"), now), Admission::Free);
        // IPv4 addresses mapped into IPv6 count as IPv4.
        assert_eq!(limits.admit(ip("192.0.2.1"), now), Admission::Free);
        assert_eq!(
            limits.admit(ip("::ffff:192.0.2.1"), now),
            Admission::Challenge
        );
    }

    #[test]
    fn a_full_table_challenges_new_networks_until_there_is_room() {
        let limits = limits(|config| config.max_tracked_networks = 2);
        let start = Instant::now();

        assert_eq!(limits.admit(ip("192.0.2.1"), start), Admission::Free);
        assert_eq!(limits.admit(ip("192.0.2.2"), start), Admission::Free);
        assert_eq!(limits.admit(ip("192.0.2.3"), start), Admission::Challenge);
        // Networks already counted go on as before.
        assert_eq!(limits.admit(ip("192.0.2.1"), start), Admission::Free);

        let later = start + Duration::from_secs(61);
        assert_eq!(limits.admit(ip("192.0.2.3"), later), Admission::Free);
    }

    fn sign_in_limits(configure: impl FnOnce(&mut config::SignIn)) -> Limits {
        let mut config = config::SignIn {
            failure_window: Duration::from_secs(60),
            max_failures_per_network: 3,
            max_tracked_networks: 100,
            ..config::SignIn::default()
        };
        configure(&mut config);
        Limits::new(&config::SignUp::default(), &config)
    }

    #[test]
    fn a_network_is_refused_sign_in_after_its_failures_until_the_window_moves_on() {
        let limits = sign_in_limits(|_| {});
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let address = ip("203.0.113.7");

        assert_eq!(limits.sign_in_refused(address, at(0)), None);
        limits.sign_in_failed(address, at(0));
        limits.sign_in_failed(address, at(10));
        // Checking doesn't count.
        assert_eq!(limits.sign_in_refused(address, at(20)), None);
        assert_eq!(limits.sign_in_refused(address, at(20)), None);
        limits.sign_in_failed(address, at(20));

        assert_eq!(
            limits.sign_in_refused(address, at(30)),
            Some(Duration::from_secs(30))
        );
        // Other networks are counted on their own.
        assert_eq!(limits.sign_in_refused(ip("203.0.113.8"), at(30)), None);
        // The first failure has left the window.
        assert_eq!(limits.sign_in_refused(address, at(60)), None);
    }

    #[test]
    fn sign_in_failures_count_by_slash_64() {
        let limits = sign_in_limits(|config| config.max_failures_per_network = 2);
        let now = Instant::now();

        limits.sign_in_failed(ip("2001:db8:1:2::1"), now);
        limits.sign_in_failed(ip("2001:db8:1:2:ffff::9"), now);

        assert!(
            limits
                .sign_in_refused(ip("2001:db8:1:2::77"), now)
                .is_some()
        );
        assert_eq!(limits.sign_in_refused(ip("2001:db8:1:3::1"), now), None);
    }

    #[test]
    fn sign_in_failures_beyond_the_tracked_networks_are_not_counted() {
        let limits = sign_in_limits(|config| {
            config.max_tracked_networks = 1;
            config.max_failures_per_network = 1;
        });
        let now = Instant::now();

        limits.sign_in_failed(ip("192.0.2.1"), now);
        limits.sign_in_failed(ip("192.0.2.2"), now);

        assert!(limits.sign_in_refused(ip("192.0.2.1"), now).is_some());
        assert_eq!(limits.sign_in_refused(ip("192.0.2.2"), now), None);
    }

    #[test]
    fn an_address_is_notified_once_per_interval() {
        let limits = limits(|config| config.notice_interval = Duration::from_secs(900));
        let start = Instant::now();
        let ann = EmailAddress::new("ann@example.com");

        assert!(limits.may_notify(&ann, start));
        assert!(!limits.may_notify(&ann, start + Duration::from_secs(899)));
        assert!(limits.may_notify(&EmailAddress::new("bob@example.com"), start));
        assert!(limits.may_notify(&ann, start + Duration::from_secs(900)));
        assert!(!limits.may_notify(&ann, start + Duration::from_secs(901)));
    }
}
