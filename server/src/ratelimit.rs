//! Rate limiting (NET-03): a token bucket in memory per user (whatever
//! session or token they come with) and per client address for requests
//! without a login. A bucket holds a quarter of a minute's requests (at
//! least 10) and refills at the per-minute rate; an empty one answers 429
//! with `Retry-After`. The limits are generous by default
//! (`CHESS_SERVER_RATE_LIMIT_PER_MINUTE`, `…_ANON_PER_MINUTE`): they stop a
//! runaway script or a flood, not a person using the API.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::error::{ApiError, ApiResult};

/// Buckets kept at most; full ones go first when there are more.
const MAX_BUCKETS: usize = 100_000;

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
pub struct RateLimits {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimits {
    /// Takes one request from `key`'s bucket at `per_minute`; refused with
    /// the seconds until there is one again.
    pub fn take(&self, key: &str, per_minute: u32) -> ApiResult<()> {
        let capacity = f64::from((per_minute / 4).max(10));
        let rate = f64::from(per_minute) / 60.0;
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("rate limit lock");
        if buckets.len() >= MAX_BUCKETS && !buckets.contains_key(key) {
            buckets.retain(|_, b| b.tokens + now.duration_since(b.last).as_secs_f64() * rate < capacity);
        }
        let bucket = buckets.entry(key.to_owned()).or_insert(Bucket { tokens: capacity, last: now });
        bucket.tokens = (bucket.tokens + now.duration_since(bucket.last).as_secs_f64() * rate).min(capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        let wait = ((1.0 - bucket.tokens) / rate).ceil().max(1.0) as u64;
        Err(ApiError::rate_limited(wait))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_hold_a_quarter_minute_and_refill() {
        let r = RateLimits::default();
        for _ in 0..15 {
            r.take("a", 60).unwrap();
        }
        let e = r.take("a", 60).unwrap_err();
        assert_eq!((e.status.as_u16(), e.retry_after), (429, Some(1)));
        r.take("b", 60).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        r.take("a", 60).unwrap();
        // At least 10 for small limits.
        for _ in 0..10 {
            r.take("c", 1).unwrap();
        }
        assert_eq!(r.take("c", 1).unwrap_err().retry_after, Some(60));
    }
}
