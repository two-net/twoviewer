//! How long to wait before reconnecting (REL-05). The delays are randomized, so that agents that
//! lost the server at the same moment do not all come back at the same moment, and increasing, so
//! that a server that is down is not flooded. The web client uses the same rule
//! (`web/src/app/shared/connection/backoff.ts`).

use std::time::Duration;

/// The longest possible delay before the first attempt.
pub const FIRST_DELAY: Duration = Duration::from_secs(1);
/// The longest possible delay before any attempt.
pub const MAX_DELAY: Duration = Duration::from_secs(30);

/// The delay before reconnection attempt `attempt`, counted from 0 since the last successful
/// connection, given `random` between 0 and 1. It is between half and all of
/// `FIRST_DELAY × 2^attempt`, at most [`MAX_DELAY`].
pub fn delay(attempt: u32, random: f64) -> Duration {
    let ceiling = FIRST_DELAY
        .saturating_mul(2_u32.saturating_pow(attempt))
        .min(MAX_DELAY);
    ceiling.mul_f64(0.5 + random.clamp(0.0, 1.0) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_are_randomized_within_half_of_their_ceiling() {
        assert_eq!(delay(0, 0.0), Duration::from_millis(500));
        assert_eq!(delay(0, 1.0), Duration::from_secs(1));
        assert_eq!(delay(2, 0.5), Duration::from_secs(3));
    }

    #[test]
    fn delays_increase_up_to_the_maximum() {
        let ceilings: Vec<_> = (0..8).map(|attempt| delay(attempt, 1.0)).collect();

        assert_eq!(
            ceilings,
            [1, 2, 4, 8, 16, 30, 30, 30].map(Duration::from_secs)
        );
        assert_eq!(delay(u32::MAX, 0.0), MAX_DELAY / 2);
    }
}
