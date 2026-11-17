//! Per-user sliding-window limiter. A zero limit disables limiting.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    per_minute: u32,
    inner: Mutex<HashMap<i64, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn allow(&self, user_id: i64) -> bool {
        if self.per_minute == 0 {
            return true;
        }
        let now = Instant::now();
        let cutoff = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        let mut users = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entries = users.entry(user_id).or_default();
        while entries.front().is_some_and(|instant| *instant < cutoff) {
            entries.pop_front();
        }
        if entries.len() >= self.per_minute as usize {
            return false;
        }
        entries.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_after_quota() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.allow(1));
        assert!(limiter.allow(1));
        assert!(!limiter.allow(1));
        assert!(limiter.allow(2));
    }

    #[test]
    fn zero_means_unlimited() {
        let limiter = RateLimiter::new(0);
        for _ in 0..10 {
            assert!(limiter.allow(1));
        }
    }
}
