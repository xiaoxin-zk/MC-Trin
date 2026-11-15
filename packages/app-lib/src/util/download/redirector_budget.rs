//! Metadata request budget for BMCLAPI-style redirectors.
//!
//! `bmclapi2.bangbang93.com` is not a byte source. Every file `GET` costs one
//! metadata request and answers `302` to a per-request signed URL on a rotating
//! OpenBMCLAPI node; those nodes measured 88–302 KB/s, two orders of magnitude
//! slower than the official hosts, and parallel connections to them do not
//! help (N=32 → 151 KB/s, ceiling ≈1.8 MB/s).
//!
//! The budget is a **fixed window of ten requests per sixty seconds** per
//! redirector origin. The service publishes the authoritative state in its own
//! headers, so those headers win over local bookkeeping whenever they are
//! present:
//!
//! * `x-ratelimit-limit-short`, `x-ratelimit-remaining-short`,
//!   `x-ratelimit-reset-short`
//! * `Retry-After-short` on a `429` (measured value: `60`)
//!
//! Overrunning the budget is not merely a `429`. A measured 46 s overrun (200
//! requests at N=8) got the client's IP TCP-blackholed for about twenty
//! minutes: SYN timeouts to every A record, with the proxy also failing. The
//! window is therefore treated as hard: a token is reserved only for a
//! *transfer start*, and while a window is exhausted nothing is probed at all.
//!
//! The long bucket (1000 requests per ~46 min) is tracked too, because it is
//! what makes a bulk install through BMCLAPI impossible: 4155 metadata requests
//! cannot fit into 1000, so an install of that shape must be planned against
//! the official hosts from the start rather than discovering the wall halfway
//! through.

use crate::util::download::constants::{
    REDIRECTOR_DEFAULT_PARK, REDIRECTOR_LONG_BUCKET_CAPACITY,
    REDIRECTOR_LONG_BUCKET_WINDOW, REDIRECTOR_MAX_PARK,
    REDIRECTOR_RATELIMIT_LIMIT_SHORT, REDIRECTOR_RATELIMIT_REMAINING_SHORT,
    REDIRECTOR_RATELIMIT_RESET_SHORT, REDIRECTOR_REFILL_WINDOW,
    REDIRECTOR_TOKEN_CAPACITY, REDIRECTOR_WAIT, RETRY_AFTER_SHORT_HEADER,
};
use crate::util::fetch::ProxyPolicy;
use parking_lot::Mutex;
use reqwest::header::HeaderMap;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Hosts that answer a file `GET` with a redirect instead of bytes.
///
/// The list is explicit: a host belongs here only when its `GET` is a metadata
/// request whose cost is independent of object size, which is what makes a
/// request budget the right limiter for it. OpenBMCLAPI node names are
/// deliberately absent — those serve bytes and are paced by
/// [`super::host_admission`], not by this budget.
const REDIRECTOR_HOSTS: &[&str] =
    &["bmclapi2.bangbang93.com", "bmclapi.bangbang93.com"];

/// How long a bulk-install plan stays in force after it is announced.
///
/// Route planning happens per file, long after the shape of the install is
/// known, so the announcement has to outlive the plan call that made it. A
/// short lease is enough: it is refreshed by every file the install starts.
const BULK_PLAN_LEASE: Duration = Duration::from_secs(120);

/// A bulk install below this many files still fits the long bucket.
const BULK_PLAN_MIN_FILES: usize = 64;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BucketKey {
    host: String,
    proxy: ProxyPolicy,
}

#[derive(Debug)]
struct Bucket {
    /// Short-window tokens: ten per sixty seconds.
    tokens: f64,
    last_refill: Instant,
    long_tokens: f64,
    long_started: Instant,
    parked_until: Option<Instant>,
    /// Concurrent transfers allowed through this redirector.
    ///
    /// Halved on every `429`, as the spec requires. It never influences the
    /// byte-serving nodes: those have their own measured limits.
    byte_limit: usize,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: REDIRECTOR_TOKEN_CAPACITY as f64,
            last_refill: now,
            long_tokens: REDIRECTOR_LONG_BUCKET_CAPACITY as f64,
            long_started: now,
            parked_until: None,
            byte_limit: REDIRECTOR_TOKEN_CAPACITY,
        }
    }

    fn refill(&mut self, now: Instant) {
        if now <= self.last_refill {
            return;
        }
        let elapsed = now.duration_since(self.last_refill);
        let window = REDIRECTOR_REFILL_WINDOW.as_secs_f64().max(1.0);
        self.tokens = (self.tokens
            + elapsed.as_secs_f64() * REDIRECTOR_TOKEN_CAPACITY as f64
                / window)
            .min(REDIRECTOR_TOKEN_CAPACITY as f64);
        self.last_refill = now;

        let long_elapsed = now.duration_since(self.long_started);
        let long_window = REDIRECTOR_LONG_BUCKET_WINDOW.as_secs_f64().max(1.0);
        self.long_tokens = (self.long_tokens
            + long_elapsed.as_secs_f64()
                * REDIRECTOR_LONG_BUCKET_CAPACITY as f64
                / long_window)
            .min(REDIRECTOR_LONG_BUCKET_CAPACITY as f64);
        self.long_started = now;
    }

    fn park_remaining(&self, now: Instant) -> Option<Duration> {
        self.parked_until
            .filter(|until| *until > now)
            .map(|until| until.saturating_duration_since(now))
    }
}

static BUCKETS: LazyLock<Mutex<HashMap<BucketKey, Bucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Why a redirector request may not be sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RedirectorDenied {
    /// The authority announced a cooldown (`429` + `retry-after-short`).
    Parked(Duration),
    /// The ten-per-minute window is spent; the next token needs this long.
    Exhausted(Duration),
    /// The long bucket cannot cover the plan.
    LongBucketExhausted,
    /// A bulk install was planned; the redirector must not be used at all.
    BulkPlanRefused,
}

impl RedirectorDenied {
    pub(crate) fn retry_after(self) -> Option<Duration> {
        match self {
            Self::Parked(duration) | Self::Exhausted(duration) => {
                Some(duration)
            }
            Self::LongBucketExhausted | Self::BulkPlanRefused => None,
        }
    }
}

impl std::fmt::Display for RedirectorDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parked(after) | Self::Exhausted(after) => write!(
                formatter,
                "metadata budget for the redirector is unavailable for {}s",
                after.as_secs()
            ),
            Self::LongBucketExhausted => write!(
                formatter,
                "the redirector's long request bucket cannot cover this plan"
            ),
            Self::BulkPlanRefused => write!(
                formatter,
                "bulk installs are planned against official hosts, not the redirector"
            ),
        }
    }
}

pub(crate) fn is_redirector(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    REDIRECTOR_HOSTS.contains(&host.as_str())
}

fn bucket_key(host: &str, proxy: ProxyPolicy) -> BucketKey {
    BucketKey {
        host: host.trim_end_matches('.').to_ascii_lowercase(),
        proxy,
    }
}

/// Bounded map of redirector buckets.
const BUCKET_TABLE_CAPACITY: usize = 64;

fn with_bucket<R>(
    host: &str,
    proxy: ProxyPolicy,
    now: Instant,
    f: impl FnOnce(&mut Bucket) -> R,
) -> R {
    let mut buckets = BUCKETS.lock();
    let key = bucket_key(host, proxy);
    if let Some(bucket) = buckets.get_mut(&key) {
        return f(bucket);
    }
    if buckets.len() >= BUCKET_TABLE_CAPACITY {
        buckets.retain(|_, bucket| {
            bucket.parked_until.is_some_and(|until| until > now)
                || now.saturating_duration_since(bucket.last_refill)
                    < REDIRECTOR_MAX_PARK
        });
    }
    if buckets.len() >= BUCKET_TABLE_CAPACITY
        && let Some(victim) = buckets
            .iter()
            .filter(|(_, bucket)| {
                // A parked bucket carries the authority's ban; dropping it
                // would let the next request probe a blackholed origin again.
                !bucket.parked_until.is_some_and(|until| until > now)
            })
            .min_by_key(|(_, bucket)| bucket.last_refill)
            .map(|(key, _)| key.clone())
    {
        buckets.remove(&victim);
    }
    if buckets.len() >= BUCKET_TABLE_CAPACITY {
        // Every entry is parked: a new authority gets a transient bucket that
        // still paces its own requests but is not remembered.
        tracing::debug!(
            capacity = BUCKET_TABLE_CAPACITY,
            "Redirector bucket table is full of parked authorities"
        );
        return f(&mut Bucket::new(now));
    }
    let bucket = buckets.entry(key).or_insert_with(|| Bucket::new(now));
    f(bucket)
}

/// Takes one metadata token without waiting.
pub(crate) fn try_acquire(
    host: &str,
    proxy: ProxyPolicy,
) -> Result<(), RedirectorDenied> {
    try_acquire_at(host, proxy, Instant::now())
}

fn try_acquire_at(
    host: &str,
    proxy: ProxyPolicy,
    now: Instant,
) -> Result<(), RedirectorDenied> {
    if bulk_plan_active() {
        return Err(RedirectorDenied::BulkPlanRefused);
    }
    with_bucket(host, proxy, now, |bucket| {
        if let Some(remaining) = bucket.park_remaining(now) {
            return Err(RedirectorDenied::Parked(remaining));
        }
        bucket.refill(now);
        if bucket.tokens < 1.0 || bucket.long_tokens < 1.0 {
            if bucket.long_tokens < 1.0 {
                return Err(RedirectorDenied::LongBucketExhausted);
            }
            let window = REDIRECTOR_REFILL_WINDOW.as_secs_f64().max(1.0);
            let needed = (1.0 - bucket.tokens) * window
                / REDIRECTOR_TOKEN_CAPACITY.max(1) as f64;
            return Err(RedirectorDenied::Exhausted(Duration::from_secs_f64(
                needed.max(0.001),
            )));
        }
        bucket.tokens -= 1.0;
        bucket.long_tokens -= 1.0;
        Ok(())
    })
}

/// Waits for a token, but only for as long as the token is actually worth.
///
/// A redirector is never the only route for Minecraft libraries or assets, so
/// waiting a whole refill window would stall an install for a minute to save a
/// metadata request. The wait is therefore precise: if the shortfall is within
/// [`REDIRECTOR_WAIT`] the caller sleeps exactly that and retries once, which
/// succeeds because the bucket refills continuously. If the shortfall is longer
/// than that — the usual case at a full window, where one token is ~6 s away —
/// the wait is pointless, so the route is reported as exhausted immediately and
/// the caller moves to the official host instead of burning the full timeout.
pub(crate) async fn acquire(
    host: &str,
    proxy: ProxyPolicy,
) -> Result<(), RedirectorDenied> {
    match try_acquire(host, proxy) {
        Ok(()) => Ok(()),
        Err(
            denied @ (RedirectorDenied::BulkPlanRefused
            | RedirectorDenied::LongBucketExhausted),
        ) => Err(denied),
        Err(denied @ RedirectorDenied::Parked(_)) => Err(denied),
        Err(RedirectorDenied::Exhausted(needed)) => {
            if needed > REDIRECTOR_WAIT {
                return Err(RedirectorDenied::Exhausted(needed));
            }
            // Sleep the real shortfall (plus a token's worth of slack so the
            // bucket cannot be a fraction short of 1.0 on the retry) rather
            // than a fixed slice that almost always undershoots.
            tokio::time::sleep(needed + REDIRECTOR_WAIT / 4).await;
            let retried = try_acquire(host, proxy);
            if let Err(denied) = &retried
                && let Some(after) = denied.retry_after()
            {
                tracing::debug!(
                    host,
                    retry_after_secs = after.as_secs_f32(),
                    "Redirector metadata budget still exhausted after waiting; \
                     the caller must fall back to an official host"
                );
            }
            retried
        }
    }
}

/// Parks a redirector for exactly the announced period.
fn park_at(
    host: &str,
    proxy: ProxyPolicy,
    duration: Duration,
    now: Instant,
) -> Duration {
    let duration = duration.clamp(Duration::from_secs(1), REDIRECTOR_MAX_PARK);
    with_bucket(host, proxy, now, |bucket| {
        bucket.parked_until = Some(now + duration);
        // A parked authority must not accumulate a full burst while it waits:
        // exactly the tokens it would have earned are the requests that caused
        // the throttle in the first place.
        bucket.tokens = bucket.tokens.min(1.0);
        duration
    })
}

/// `retry-after-short` is BMCLAPI's own hint and is deliberately not the
/// standard `Retry-After`: the redirector answers `Retry-After: 0` in places
/// while asking for a full minute through this header.
///
/// Only an integer number of seconds is accepted. An HTTP-date is rejected
/// rather than parsed: BMCLAPI never sends one, and a misparsed date would
/// either park for years or not at all.
pub(crate) fn parse_retry_after_short(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER_SHORT_HEADER)?.to_str().ok()?;
    let seconds = value.trim().parse::<u64>().ok()?;
    Some(
        Duration::from_secs(seconds)
            .clamp(Duration::from_secs(1), REDIRECTOR_MAX_PARK),
    )
}

fn header_usize(headers: &HeaderMap, name: &str) -> Option<usize> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()
}

/// Applies the authoritative short-window headers to a live bucket.
///
/// The service's own `remaining` and `reset` win over the local refill
/// estimate: local bookkeeping can only drift downward from truth (a request
/// that failed before reaching the redirector still consumed a token here), so
/// trusting a larger `remaining` from the server is always safe and trusting a
/// smaller one is conservative.
fn apply_announced(bucket: &mut Bucket, headers: &HeaderMap, now: Instant) {
    if let Some(remaining) =
        header_usize(headers, REDIRECTOR_RATELIMIT_REMAINING_SHORT)
    {
        let limit = header_usize(headers, REDIRECTOR_RATELIMIT_LIMIT_SHORT)
            .unwrap_or(REDIRECTOR_TOKEN_CAPACITY);
        let remaining = remaining.min(limit) as f64;
        bucket.tokens = remaining;
    }
    if headers
        .get(REDIRECTOR_RATELIMIT_RESET_SHORT)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
    {
        // The header is a window length in seconds and the window restarts
        // from this response, which is exactly the semantics of a fixed window:
        // the refill clock is reset so no stale elapsed time is credited to the
        // remaining count that `x-ratelimit-remaining-short` just announced.
        bucket.last_refill = now;
    }
}

/// Records a redirector response.
///
/// A `2xx`/`3xx` applies the announced budget. A `429` parks the authority for
/// exactly the period it announced (default sixty seconds) and halves the byte
/// limit. Nothing here changes connection admission for the node behind the
/// redirector: the redirector says nothing about that server's capacity.
pub(crate) fn note_response(
    host: &str,
    proxy: ProxyPolicy,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
) {
    note_response_at(host, proxy, status, headers, Instant::now());
}

/// [`note_response`] against an explicit clock.
///
/// The park deadline is an instant, so the period it covers can only be checked
/// against the instant the announcement was recorded. Reading the wall clock
/// here and querying from a different one made the announced period look longer
/// than it is by exactly the time between the two readings.
fn note_response_at(
    host: &str,
    proxy: ProxyPolicy,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    now: Instant,
) {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let duration =
            parse_retry_after_short(headers).unwrap_or(REDIRECTOR_DEFAULT_PARK);
        park_at(host, proxy, duration, now);
        with_bucket(host, proxy, now, |bucket| {
            bucket.byte_limit = (bucket.byte_limit / 2).max(1);
        });
        tracing::debug!(
            host,
            byte_limit = byte_limit_at(host, proxy, now),
            "Redirector rate limited; halving its byte allowance"
        );
        return;
    }
    if status.is_success() || status.is_redirection() {
        with_bucket(host, proxy, now, |bucket| {
            bucket.refill(now);
            apply_announced(bucket, headers, now);
        });
    }
}

/// Current byte-transfer allowance through a redirector.
///
/// Starts at the ten-per-minute window and halves on every `429`, so a
/// throttled redirector admits fewer parallel file GETs rather than the same
/// number at a slower pace. It is reported here rather than enforced in
/// [`acquire`] because a single file GET is one token either way: the halving
/// exists to stop a wide batch from re-filling the window it just overran.
pub(crate) fn byte_limit(host: &str, proxy: ProxyPolicy) -> usize {
    byte_limit_at(host, proxy, Instant::now())
}

fn byte_limit_at(host: &str, proxy: ProxyPolicy, now: Instant) -> usize {
    with_bucket(host, proxy, now, |bucket| bucket.byte_limit)
}

/// Whether a plan of `requests` metadata calls can fit the long bucket.
///
/// The long bucket holds [`REDIRECTOR_LONG_BUCKET_CAPACITY`] requests per
/// [`REDIRECTOR_LONG_BUCKET_WINDOW`]. A plan above that cannot go through the
/// redirector however the short window is paced, so planning refuses it rather
/// than spending the first ten files discovering the wall.
pub(crate) fn long_bucket_can_plan(
    host: &str,
    proxy: ProxyPolicy,
    requests: usize,
) -> bool {
    let now = Instant::now();
    with_bucket(host, proxy, now, |bucket| {
        bucket.refill(now);
        (bucket.long_tokens as usize) >= requests
    })
}

/// Announces a bulk install: `files` metadata requests would be needed if the
/// batch went through the redirector.
///
/// Planning is refused outright rather than degraded, because the long bucket
/// cannot cover it and the failure mode of discovering that halfway through is
/// the measured IP ban. The lease is refreshed by every started file, so it
/// expires on its own after the install stops.
pub(crate) fn note_plan(files: usize) {
    if files < BULK_PLAN_MIN_FILES {
        return;
    }
    let covers =
        long_bucket_can_plan(REDIRECTOR_HOSTS[0], ProxyPolicy::System, files);
    *BULK_PLAN_UNTIL.lock() = Some(Instant::now() + BULK_PLAN_LEASE);
    tracing::debug!(
        files,
        long_bucket_covers_plan = covers,
        %REDIRECTOR_LONG_BUCKET_CAPACITY,
        "Planning a bulk install; BMCLAPI is excluded as a file route"
    );
}

static BULK_PLAN_UNTIL: LazyLock<Mutex<Option<Instant>>> =
    LazyLock::new(|| Mutex::new(None));

/// Whether a bulk install is currently the plan being built.
pub(crate) fn bulk_plan_active() -> bool {
    let mut lease = BULK_PLAN_UNTIL.lock();
    match *lease {
        Some(until) if until > Instant::now() => true,
        Some(_) => {
            *lease = None;
            false
        }
        None => false,
    }
}

#[cfg(test)]
pub(crate) fn clear_for_test() {
    BUCKETS.lock().clear();
    *BULK_PLAN_UNTIL.lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::ReentrantMutex;
    use reqwest::header::HeaderValue;

    /// The bucket map is process-wide, so one lock serialises this module's
    /// tests: clearing it mid-run would turn "the eleventh request is refused"
    /// into a flake.
    static TEST_LOCK: LazyLock<ReentrantMutex<()>> =
        LazyLock::new(Default::default);

    fn guard() -> parking_lot::ReentrantMutexGuard<'static, ()> {
        TEST_LOCK.lock()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn a_burst_of_ten_is_allowed_and_the_eleventh_is_refused() {
        let _guard = guard();
        clear_for_test();
        let start = Instant::now();
        for index in 0..REDIRECTOR_TOKEN_CAPACITY {
            assert!(
                try_acquire_at(
                    "bmclapi2.bangbang93.com",
                    ProxyPolicy::Direct,
                    start
                )
                .is_ok(),
                "token {index} must be available"
            );
        }
        let denied = try_acquire_at(
            "bmclapi2.bangbang93.com",
            ProxyPolicy::Direct,
            start,
        )
        .unwrap_err();
        assert!(matches!(denied, RedirectorDenied::Exhausted(_)));
        // Ten tokens per minute: the next one is ~6 s away.
        assert!(denied.retry_after().unwrap() > Duration::from_secs(5));
    }

    #[test]
    fn tokens_refill_at_ten_per_minute() {
        let _guard = guard();
        clear_for_test();
        let start = Instant::now();
        for _ in 0..REDIRECTOR_TOKEN_CAPACITY {
            try_acquire_at("refill.example", ProxyPolicy::Direct, start)
                .unwrap();
        }
        // Half a window earns five tokens.
        let half = start + REDIRECTOR_REFILL_WINDOW / 2;
        for _ in 0..5 {
            try_acquire_at("refill.example", ProxyPolicy::Direct, half)
                .unwrap();
        }
        assert!(
            try_acquire_at("refill.example", ProxyPolicy::Direct, half)
                .is_err()
        );
    }

    #[test]
    fn retry_after_short_parses_seconds_and_rejects_the_standard_header() {
        assert_eq!(
            parse_retry_after_short(&headers(&[("retry-after-short", "90")])),
            Some(Duration::from_secs(90))
        );
        assert_eq!(parse_retry_after_short(&HeaderMap::new()), None);
        assert_eq!(
            parse_retry_after_short(&headers(&[("retry-after-short", "soon")])),
            None
        );
        assert_eq!(
            parse_retry_after_short(&headers(&[("retry-after", "120")])),
            None,
            "only the redirector-specific header is authoritative"
        );
    }

    #[test]
    fn a_429_parks_exactly_the_announced_period_and_halves_the_byte_limit() {
        let _guard = guard();
        clear_for_test();
        let now = Instant::now();
        let host = "parked.example";
        // The park deadline is an instant, so the announcement has to be
        // recorded against the same clock the assertion queries: reading two
        // wall clocks would add the gap between them to the announced period.
        note_response_at(
            host,
            ProxyPolicy::Direct,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after-short", "62")]),
            now,
        );
        assert_eq!(
            byte_limit_at(host, ProxyPolicy::Direct, now),
            REDIRECTOR_TOKEN_CAPACITY / 2
        );
        let denied =
            try_acquire_at(host, ProxyPolicy::Direct, now).unwrap_err();
        assert!(matches!(denied, RedirectorDenied::Parked(_)));
        assert_eq!(
            denied.retry_after(),
            Some(Duration::from_secs(62)),
            "the announced period is the park, neither longer nor shorter"
        );
        // The park expires on its own; it is not a permanent ban.
        assert!(
            try_acquire_at(
                host,
                ProxyPolicy::Direct,
                now + Duration::from_secs(63)
            )
            .is_ok()
        );
    }

    #[test]
    fn an_absent_header_defaults_to_sixty_seconds() {
        let _guard = guard();
        clear_for_test();
        let start = Instant::now();
        let duration = REDIRECTOR_DEFAULT_PARK;
        assert_eq!(duration, Duration::from_secs(60));
        park_at("default-park.example", ProxyPolicy::Direct, duration, start);
        let denied =
            try_acquire_at("default-park.example", ProxyPolicy::Direct, start)
                .unwrap_err();
        assert!(denied.retry_after().unwrap() <= Duration::from_secs(60));
        assert!(denied.retry_after().unwrap() > Duration::from_secs(59));
    }

    #[test]
    fn the_announced_headers_override_local_bookkeeping() {
        let _guard = guard();
        clear_for_test();
        let start = Instant::now();
        let host = "announced.example";
        for _ in 0..REDIRECTOR_TOKEN_CAPACITY {
            try_acquire_at(host, ProxyPolicy::Direct, start).unwrap();
        }
        // The service says two requests remain in the current window.
        note_response(
            host,
            ProxyPolicy::Direct,
            reqwest::StatusCode::OK,
            &headers(&[
                ("x-ratelimit-limit-short", "10"),
                ("x-ratelimit-remaining-short", "2"),
                ("x-ratelimit-reset-short", "60"),
            ]),
        );
        assert!(try_acquire_at(host, ProxyPolicy::Direct, start).is_ok());
        assert!(try_acquire_at(host, ProxyPolicy::Direct, start).is_ok());
        assert!(try_acquire_at(host, ProxyPolicy::Direct, start).is_err());
    }

    #[test]
    fn aliases_share_one_bucket_per_host() {
        let _guard = guard();
        clear_for_test();
        let start = Instant::now();
        for _ in 0..REDIRECTOR_TOKEN_CAPACITY {
            try_acquire_at(
                "bmclapi2.bangbang93.com.",
                ProxyPolicy::Direct,
                start,
            )
            .unwrap();
        }
        assert!(
            try_acquire_at(
                "BMCLAPI2.bangbang93.com",
                ProxyPolicy::Direct,
                start
            )
            .is_err(),
            "a trailing dot or different case is the same redirector"
        );
        assert!(is_redirector("bmclapi.bangbang93.com"));
        assert!(!is_redirector("bmclapi8.sakura.sld.tw"));
    }

    #[test]
    fn the_long_bucket_refuses_a_plan_it_cannot_cover() {
        let _guard = guard();
        clear_for_test();
        assert!(long_bucket_can_plan(
            "plan.example",
            ProxyPolicy::Direct,
            REDIRECTOR_LONG_BUCKET_CAPACITY
        ));
        assert!(!long_bucket_can_plan(
            "plan.example",
            ProxyPolicy::Direct,
            4155
        ));
    }

    #[test]
    fn a_bulk_plan_refuses_the_redirector_as_a_route() {
        let _guard = guard();
        clear_for_test();
        assert!(!bulk_plan_active());
        note_plan(400);
        assert!(bulk_plan_active());
        clear_for_test();
        note_plan(BULK_PLAN_MIN_FILES - 1);
        assert!(!bulk_plan_active());
    }
}
