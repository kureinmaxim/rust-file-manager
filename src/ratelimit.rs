//! Fixed-window attempt counter for password checks reachable from the Mini App.
//!
//! Keys are Telegram user ids taken from verified initData, so an attacker
//! needs a real Telegram account per bucket; IP addresses behind nginx are
//! not reliable enough to key on.

use std::collections::HashMap;
use std::sync::Mutex;

pub struct RateLimiter {
    max_attempts: u32,
    window_secs: u64,
    buckets: Mutex<HashMap<String, (u32, u64)>>,
}

impl RateLimiter {
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            max_attempts,
            window_secs,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Count an attempt; false when the key is over its limit for this window.
    pub fn allow(&self, key: &str, now: u64) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter lock");
        // Drop stale buckets so the map cannot grow without bound.
        if buckets.len() > 10_000 {
            buckets.retain(|_, (_, start)| now.saturating_sub(*start) < self.window_secs);
        }
        let entry = buckets.entry(key.to_string()).or_insert((0, now));
        if now.saturating_sub(entry.1) >= self.window_secs {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_key_and_resets_after_window() {
        let limiter = RateLimiter::new(2, 60);
        assert!(limiter.allow("a", 100));
        assert!(limiter.allow("a", 101));
        assert!(!limiter.allow("a", 102));
        assert!(limiter.allow("b", 102));
        assert!(limiter.allow("a", 160));
    }
}
