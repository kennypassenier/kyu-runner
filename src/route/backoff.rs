//! Waiting between attempts: an exponential backoff with jitter, and a
//! sleep that a shutdown signal cuts short.

use std::time::Duration;

use tokio::sync::watch;

/// Jitter adds up to 1/JITTER_DIVISOR of the current step, enough to
/// spread retries from many routes without a rand dependency.
const JITTER_DIVISOR: u64 = 8;

/// Exponential backoff. The jitter comes from the nanosecond clock —
/// enough to spread retries, no rand dependency.
pub(super) struct Backoff {
    next_ms: u64,
    base_ms: u64,
    cap_ms: u64,
}

impl Backoff {
    pub(super) fn new(base_ms: u64, cap_ms: u64) -> Self {
        Self {
            next_ms: base_ms,
            base_ms,
            cap_ms,
        }
    }

    pub(super) fn next(&mut self) -> Duration {
        let ms = self.next_ms;
        self.next_ms = (self.next_ms * 2).min(self.cap_ms);
        // A clock before the epoch only loses the jitter, never the wait.
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| u64::from(now.subsec_nanos()))
            .unwrap_or(0)
            % (ms / JITTER_DIVISOR + 1);
        Duration::from_millis(ms + jitter)
    }

    pub(super) fn reset(&mut self) {
        self.next_ms = self.base_ms;
    }
}

/// Sleep that a shutdown signal cuts short. Returns true on shutdown.
pub(super) async fn interrupted(delay: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = shutdown.changed() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::Backoff;

    #[test]
    fn l3_ar6_backoff_doubles_to_its_cap_with_bounded_jitter() {
        let mut backoff = Backoff::new(500, 30_000);
        let mut expected = 500u64;
        for _ in 0..10 {
            let delay = backoff.next().as_millis() as u64;
            // Jitter adds at most an eighth of the base step.
            assert!(
                delay >= expected && delay <= expected + expected / 8 + 1,
                "delay {delay} outside [{expected}, +12.5%]"
            );
            expected = (expected * 2).min(30_000);
        }
        assert_eq!(expected, 30_000, "the cap is reached and held");
    }

    #[test]
    fn l3_ar6_reset_returns_to_the_base() {
        let mut backoff = Backoff::new(1_000, 60_000);
        for _ in 0..8 {
            backoff.next();
        }
        backoff.reset();
        let delay = backoff.next().as_millis() as u64;
        assert!((1_000..=1_126).contains(&delay), "reset delay {delay}");
    }
}
