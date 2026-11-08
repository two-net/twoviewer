//! Limits per network, kept in memory: how often one network may sign up (AUTH-15), fail to sign
//! in (SEC-08) and ask for password resets (SEC-08); how often sign-ups and password resets may
//! email one address; and how often one account may ask to change its email address (AUTH-11).
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

use sea_orm::prelude::Uuid;
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
    password_reset: config::PasswordReset,
    email_change: config::EmailChange,
    /// Sign-ups per network.
    sign_ups: Windows<Network>,
    /// Failed sign-ins per network.
    sign_in_failures: Windows<Network>,
    /// Password reset requests per network.
    reset_requests: Windows<Network>,
    /// Email change requests per account.
    email_changes: Windows<Uuid>,
    /// When sign-up last emailed each address.
    sign_up_notices: Notices,
    /// When password reset last emailed each address.
    reset_emails: Notices,
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

/// The times of events per key within a sliding `window`, such as sign-ups per network, for at
/// most `max_tracked` keys at once.
struct Windows<K> {
    window: Duration,
    max_tracked: usize,
    keys: Mutex<Counts<K, VecDeque<Instant>>>,
}

impl<K: Copy + Eq + std::hash::Hash> Windows<K> {
    fn new(window: Duration, max_tracked: usize) -> Self {
        Self {
            window,
            max_tracked,
            keys: Mutex::new(Counts::new()),
        }
    }

    /// Runs `f` on the events of `key` within the window at `now`, oldest first, which it may add
    /// to. Returns `None` without running it when the table is full and `key` isn't in it, even
    /// after the keys without recent events have been removed.
    fn update<R>(
        &self,
        key: K,
        now: Instant,
        f: impl FnOnce(&mut VecDeque<Instant>) -> R,
    ) -> Option<R> {
        let window = self.window;
        let expired = |events: &VecDeque<Instant>| {
            events
                .back()
                .is_none_or(|last| now.saturating_duration_since(*last) >= window)
        };
        let mut keys = self.keys.lock().expect("never poisoned");
        keys.prune(now, window, expired);
        if !keys.entries.contains_key(&key) && keys.entries.len() >= self.max_tracked {
            keys.prune(now, FULL_PRUNE_INTERVAL, expired);
            if keys.entries.len() >= self.max_tracked {
                return None;
            }
        }
        let events = keys.entries.entry(key).or_default();
        while events
            .front()
            .is_some_and(|first| now.saturating_duration_since(*first) >= window)
        {
            events.pop_front();
        }
        Some(f(events))
    }

    /// How many events `key` had within the window at `now`, and how long until the oldest of them
    /// leaves it. It changes nothing.
    fn count(&self, key: K, now: Instant) -> (usize, Duration) {
        let keys = self.keys.lock().expect("never poisoned");
        let Some(events) = keys.entries.get(&key) else {
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

/// When each address was last emailed, by the SHA-256 of the address, so that the addresses
/// themselves aren't kept, and whether it may be emailed again.
struct Notices {
    interval: Duration,
    last: Mutex<Counts<[u8; 32], Instant>>,
}

impl Notices {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(Counts::new()),
        }
    }

    /// Whether `address` may be emailed at `now`, at most once per interval. If it may, the time
    /// is recorded.
    fn may_send(&self, address: &EmailAddress, now: Instant) -> bool {
        let interval = self.interval;
        let key: [u8; 32] = Sha256::digest(address.as_str().as_bytes()).into();
        let mut last = self.last.lock().expect("never poisoned");
        last.prune(now, interval, |at| {
            now.saturating_duration_since(*at) >= interval
        });
        match last.entries.get(&key) {
            Some(at) if now.saturating_duration_since(*at) < interval => false,
            _ => {
                last.entries.insert(key, now);
                true
            }
        }
    }
}

impl Limits {
    pub fn new(
        sign_up: &config::SignUp,
        sign_in: &config::SignIn,
        password_reset: &config::PasswordReset,
        email_change: &config::EmailChange,
    ) -> Self {
        Self {
            sign_up: sign_up.clone(),
            sign_in: sign_in.clone(),
            password_reset: password_reset.clone(),
            email_change: email_change.clone(),
            sign_ups: Windows::new(sign_up.window, sign_up.max_tracked_networks),
            sign_in_failures: Windows::new(sign_in.failure_window, sign_in.max_tracked_networks),
            reset_requests: Windows::new(
                password_reset.window,
                password_reset.max_tracked_networks,
            ),
            // Each entry follows a recent re-authentication, which takes a password hash, so
            // that limits how fast they come, as for `Notices`.
            email_changes: Windows::new(email_change.window, usize::MAX),
            sign_up_notices: Notices::new(sign_up.notice_interval),
            reset_emails: Notices::new(password_reset.email_interval),
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
        self.sign_up_notices.may_send(address, now)
    }

    /// Counts a password reset request from `address` at `now`, unless its network has already
    /// made `password_reset.max_per_network` within `password_reset.window`: then it is refused,
    /// and a new request is possible after the returned time. While too many networks are counted,
    /// requests from others aren't: the limit per address ([`Limits::may_email_reset`]) still
    /// holds.
    pub fn admit_reset(&self, address: IpAddr, now: Instant) -> Result<(), Duration> {
        let max = self.password_reset.max_per_network;
        self.reset_requests
            .update(Network::from(address), now, |requests| {
                if requests.len() >= max {
                    return Err(self.reset_requests.retry_after(requests, now));
                }
                requests.push_back(now);
                Ok(())
            })
            .unwrap_or(Ok(()))
    }

    /// Whether a password reset at `now` may email `address`, at most once per
    /// `password_reset.email_interval`. If it may, the time is recorded.
    ///
    /// Ask only for addresses that have an account, so that the table holds at most one entry per
    /// account.
    pub fn may_email_reset(&self, address: &EmailAddress, now: Instant) -> bool {
        self.reset_emails.may_send(address, now)
    }

    /// Counts a request of `user_id` at `now` to change their email address, unless they have
    /// already made `email_change.max_per_account` within `email_change.window`: then it is
    /// refused, and a new request is possible after the returned time.
    pub fn admit_email_change(&self, user_id: Uuid, now: Instant) -> Result<(), Duration> {
        let max = self.email_change.max_per_account;
        self.email_changes
            .update(user_id, now, |requests| {
                if requests.len() >= max {
                    return Err(self.email_changes.retry_after(requests, now));
                }
                requests.push_back(now);
                Ok(())
            })
            .unwrap_or(Ok(()))
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
        Limits::new(
            &config,
            &config::SignIn::default(),
            &config::PasswordReset::default(),
            &config::EmailChange::default(),
        )
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
        Limits::new(
            &config::SignUp::default(),
            &config,
            &config::PasswordReset::default(),
            &config::EmailChange::default(),
        )
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

    fn reset_limits(configure: impl FnOnce(&mut config::PasswordReset)) -> Limits {
        let mut config = config::PasswordReset {
            window: Duration::from_secs(60),
            max_per_network: 3,
            max_tracked_networks: 100,
            email_interval: Duration::from_secs(300),
            ..config::PasswordReset::default()
        };
        configure(&mut config);
        Limits::new(
            &config::SignUp::default(),
            &config::SignIn::default(),
            &config,
            &config::EmailChange::default(),
        )
    }

    #[test]
    fn a_network_is_refused_password_resets_past_the_maximum_until_the_window_moves_on() {
        let limits = reset_limits(|_| {});
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let address = ip("203.0.113.7");

        assert_eq!(limits.admit_reset(address, at(0)), Ok(()));
        assert_eq!(limits.admit_reset(address, at(10)), Ok(()));
        assert_eq!(limits.admit_reset(address, at(20)), Ok(()));
        assert_eq!(
            limits.admit_reset(address, at(30)),
            Err(Duration::from_secs(30))
        );
        // Refused requests don't count.
        assert_eq!(
            limits.admit_reset(address, at(40)),
            Err(Duration::from_secs(20))
        );
        // Other networks are counted on their own.
        assert_eq!(limits.admit_reset(ip("203.0.113.8"), at(40)), Ok(()));
        // The first request has left the window.
        assert_eq!(limits.admit_reset(address, at(60)), Ok(()));
    }

    #[test]
    fn password_resets_count_by_slash_64() {
        let limits = reset_limits(|config| config.max_per_network = 1);
        let now = Instant::now();

        assert_eq!(limits.admit_reset(ip("2001:db8:1:2::1"), now), Ok(()));
        assert!(limits.admit_reset(ip("2001:db8:1:2:ffff::9"), now).is_err());
        assert_eq!(limits.admit_reset(ip("2001:db8:1:3::1"), now), Ok(()));
    }

    #[test]
    fn password_resets_beyond_the_tracked_networks_are_not_counted() {
        let limits = reset_limits(|config| {
            config.max_tracked_networks = 1;
            config.max_per_network = 1;
        });
        let now = Instant::now();

        assert_eq!(limits.admit_reset(ip("192.0.2.1"), now), Ok(()));
        assert!(limits.admit_reset(ip("192.0.2.1"), now).is_err());
        assert_eq!(limits.admit_reset(ip("192.0.2.2"), now), Ok(()));
        assert_eq!(limits.admit_reset(ip("192.0.2.2"), now), Ok(()));
    }

    #[test]
    fn an_address_gets_a_reset_email_once_per_interval_apart_from_sign_up() {
        let limits = reset_limits(|_| {});
        let start = Instant::now();
        let ann = EmailAddress::new("ann@example.com");

        assert!(limits.may_email_reset(&ann, start));
        assert!(!limits.may_email_reset(&ann, start + Duration::from_secs(299)));
        // Sign-up's emails are counted on their own.
        assert!(limits.may_notify(&ann, start));
        assert!(limits.may_email_reset(&ann, start + Duration::from_secs(300)));
    }

    #[test]
    fn an_account_is_refused_email_changes_past_the_maximum_until_the_window_moves_on() {
        let limits = Limits::new(
            &config::SignUp::default(),
            &config::SignIn::default(),
            &config::PasswordReset::default(),
            &config::EmailChange {
                window: Duration::from_secs(60),
                max_per_account: 2,
            },
        );
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let (ann, bob) = (Uuid::new_v4(), Uuid::new_v4());

        assert_eq!(limits.admit_email_change(ann, at(0)), Ok(()));
        assert_eq!(limits.admit_email_change(ann, at(10)), Ok(()));
        assert_eq!(
            limits.admit_email_change(ann, at(20)),
            Err(Duration::from_secs(40))
        );
        // Refused requests don't count, and other accounts are counted on their own.
        assert_eq!(limits.admit_email_change(bob, at(20)), Ok(()));
        // The first request has left the window.
        assert_eq!(limits.admit_email_change(ann, at(60)), Ok(()));
    }
}
