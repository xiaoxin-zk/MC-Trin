//! Adaptive byte admission per post-redirect host.
//!
//! A single global concurrency number is wrong in both directions on the
//! measured network: `piston-data.mojang.com` sustained 694–722 Mbps at 32–96
//! connections with no knee, `libraries.minecraft.net` was clean to 96, while
//! `maven.aliyun.com` kneed at 8–16 and `cmcc.mirrors.ustc.edu.cn` answered
//! `ConnectionRefused` from 16 up (731 RSTs at N=96). The unit of admission is
//! therefore the **post-redirect** `(scheme, host, port)` — never a redirector
//! such as `bmclapi2.bangbang93.com`, which only hands out signed URLs and
//! would teach the controller about a load balancer instead of a server.
//!
//! Two knee signals are used, selected by object size because they measure
//! different things:
//!
//! * Objects under [`SMALL_FILE_SIGNAL_BYTES`] are latency-bound: measured p90
//!   TTFB went 120 ms → 722 ms between 32 and 64 streams on 121 KB assets, so
//!   p90 TTFB inflation past 1.25× the host's own baseline is the knee.
//! * Objects at or above [`BULK_SIGNAL_BYTES`] are throughput-bound, so a
//!   throughput plateau is the knee.
//!
//! Growth is additive and gated by an app-limited guard: if fewer than half of
//! the allowed transfers are actually in flight, the limit is not the
//! bottleneck and a latency increase is self-inflicted queueing, not
//! congestion.

use crate::util::download::constants::{
    GLOBAL_IN_FLIGHT_CAP, H2_STREAMS_PER_CONNECTION_CAP,
    H2_STREAMS_PER_LEARNED_SOCKET, HOST_CONCURRENCY_CAP_MEASURED_MIN,
    HOST_CONCURRENCY_CAP_UNMEASURED, HOST_CONCURRENCY_COOLDOWN_LADDER,
    HOST_CONCURRENCY_FAST_ATTACK_PERCENT, HOST_CONCURRENCY_FAST_GROWTH_CEILING,
    HOST_CONCURRENCY_FAST_GROWTH_STEP, HOST_CONCURRENCY_FLOOR,
    HOST_CONCURRENCY_GROWTH_PER_COMPLETIONS, HOST_CONCURRENCY_GROWTH_STEP,
    HOST_CONCURRENCY_INITIAL, HOST_CONCURRENCY_LATENCY_RATIO_PERCENT,
    HOST_CONCURRENCY_PERCENT_BASE, SMALL_FILE_CONCURRENCY_CEILING,
    SMALL_FILE_SIGNAL_BYTES,
};
use crate::util::fetch::{DownloadRoute, ProxyPolicy};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::bounded_table::BoundedTable;

/// Latency samples kept per host for the p90 estimate.
const LATENCY_SAMPLES: usize = 32;
/// Minimum samples before a p90 is trusted as a baseline.
const LATENCY_BASELINE_SAMPLES: usize = 8;
/// Low-concurrency p90 estimates the baseline remembers.
///
/// The baseline is their minimum, so this window is how long one anomalously
/// fast estimate can hold the reference down: at one estimate per latency
/// sample, sixteen estimates is twice the p90 window, which is long enough to
/// average out a single outlier and short enough that a genuine improvement to
/// the network path is picked up within a session.
const BASELINE_WINDOW: usize = 16;
/// A throughput window shorter than this is not evidence of a plateau.
const RATE_WINDOW_MIN: Duration = Duration::from_millis(500);
/// Growth stops after this many consecutive non-improving rate windows.
const PLATEAU_WINDOWS: u8 = 2;
/// Improvement below this percentage counts as a plateau.
const PLATEAU_GAIN_PERCENT: u128 = 105;
/// A failure streak older than this is not consecutive any more.
const STRIKE_DECAY: Duration = Duration::from_secs(60);
/// Strikes before the cooldown ladder engages.
const STRIKES_BEFORE_LADDER: u8 = 3;

/// How an object's size selects its knee signal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SizeClass {
    /// Latency-bound: p90 TTFB inflation is the signal, ceiling 32.
    Small,
    /// Neither signal is reliable; growth only counts completions.
    Medium,
    /// Throughput-bound: a plateau is the signal.
    Bulk,
}

impl SizeClass {
    pub(crate) fn of(size: Option<u64>) -> Self {
        match size {
            Some(bytes) if bytes < SMALL_FILE_SIGNAL_BYTES => Self::Small,
            Some(bytes) if bytes >= super::constants::BULK_SIGNAL_BYTES => {
                Self::Bulk
            }
            _ => Self::Medium,
        }
    }

    /// Ceiling for this object size, given the host's own measured cap.
    ///
    /// The small-file ceiling of 32 is where 400-file batches peaked
    /// (178.9 files/s) before declining at 64 and 96; it applies on top of
    /// whatever the host earned for bulk transfer.
    ///
    /// `Medium` — which includes an *unknown* size — is clamped to that same
    /// 32 rather than to the bulk cap. Without a byte count there is no
    /// throughput evidence to be had, so growth can only come from completions,
    /// and letting an unmeasured object claim the full bulk ceiling would hand a
    /// host of unknown capacity a plan nothing has justified.
    pub(crate) fn ceiling(self, host_cap: usize) -> usize {
        match self {
            Self::Small | Self::Medium => {
                host_cap.min(SMALL_FILE_CONCURRENCY_CEILING)
            }
            Self::Bulk => host_cap,
        }
    }
}

struct HostState {
    limit: usize,
    in_flight: usize,
    /// Multiplexed streams currently admitted against this host.
    ///
    /// Separate from `in_flight` because the two measure different things: a
    /// stream is not a socket. Measured, one connection carrying sixteen
    /// streams ties eight plain connections, so one socket is worth two streams
    /// — charging each stream as a whole transfer slot would hand a fresh host
    /// four streams where four plain sockets already deliver eight.
    stream_in_flight: usize,
    /// `None` while healthy; `Some` until the cooldown elapses.
    cooldown_until: Option<Instant>,
    /// The ladder was exhausted; this host is done for the session.
    blocked: bool,
    completions: u32,
    latencies: VecDeque<f64>,
    baseline_p90_ms: Option<f64>,
    /// Recent low-concurrency p90 estimates the baseline is the minimum of.
    baseline_history: VecDeque<f64>,
    window_started: Instant,
    window_bytes: u64,
    last_rate: Option<u64>,
    plateau_windows: u8,
    strikes: u8,
    last_strike_at: Option<Instant>,
    ladder_step: u8,
    /// Last rate this host actually delivered, for the UI.
    observed_rate: Option<u64>,
}

impl HostState {
    fn new(now: Instant) -> Self {
        Self {
            limit: HOST_CONCURRENCY_INITIAL,
            in_flight: 0,
            stream_in_flight: 0,
            cooldown_until: None,
            blocked: false,
            completions: 0,
            latencies: VecDeque::with_capacity(LATENCY_SAMPLES),
            baseline_p90_ms: None,
            baseline_history: VecDeque::with_capacity(BASELINE_WINDOW),
            window_started: now,
            window_bytes: 0,
            last_rate: None,
            plateau_windows: 0,
            strikes: 0,
            last_strike_at: None,
            ladder_step: 0,
            observed_rate: None,
        }
    }

    fn parked(&self, now: Instant) -> bool {
        self.blocked || self.cooldown_until.is_some_and(|until| until > now)
    }

    fn effective_limit(&self, size: SizeClass, host_cap: usize) -> usize {
        self.limit.min(size.ceiling(host_cap)).max(1)
    }

    fn p90_ms(&self) -> Option<f64> {
        if self.latencies.len() < LATENCY_BASELINE_SAMPLES {
            return None;
        }
        let mut sorted: Vec<f64> = self.latencies.iter().copied().collect();
        sorted.sort_by(|left, right| {
            left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Nearest-rank p90, clamped so a short ring still indexes in bounds.
        let index = ((sorted.len() as f64 * 0.9).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1);
        sorted.get(index).copied()
    }

    fn latency_within_baseline(&self) -> bool {
        match (self.p90_ms(), self.baseline_p90_ms) {
            (Some(p90), Some(baseline)) if baseline > 0.0 => {
                (p90 * HOST_CONCURRENCY_PERCENT_BASE as f64)
                    <= baseline * HOST_CONCURRENCY_LATENCY_RATIO_PERCENT as f64
            }
            _ => true,
        }
    }

    fn push_latency(&mut self, latency_ms: f64) {
        if !latency_ms.is_finite() || latency_ms < 0.0 {
            return;
        }
        if self.latencies.len() >= LATENCY_SAMPLES {
            self.latencies.pop_front();
        }
        self.latencies.push_back(latency_ms);
        // The baseline is learned at low concurrency, which is where the knee
        // is. Learning it at 64 streams would define the congested latency as
        // normal and make the controller blind.
        if self.limit <= HOST_CONCURRENCY_FAST_GROWTH_CEILING
            && let Some(p90) = self.p90_ms()
        {
            self.remember_baseline(p90);
        }
    }

    /// Refreshes the latency baseline from a p90 measured at low concurrency.
    ///
    /// The reference is the *lowest* recent p90, not the lowest p90 ever seen.
    /// A monotone minimum is not safe here: one anomalous fast sample — a
    /// cache hit, a reused connection whose TLS handshake the other samples
    /// paid for — would pin the floor below anything the host can repeat, so
    /// [`Self::latency_within_baseline`] would answer `false` for the rest of
    /// the session and the limit would never grow again. Keeping the minimum
    /// over a bounded window of recent estimates preserves the intent (a knee
    /// is judged against this host's own best *sustained* latency) while
    /// letting the floor forget an outlier after [`BASELINE_WINDOW`] further
    /// measurements.
    fn remember_baseline(&mut self, p90: f64) {
        if self.baseline_history.len() >= BASELINE_WINDOW {
            self.baseline_history.pop_front();
        }
        self.baseline_history.push_back(p90);
        self.baseline_p90_ms = self.baseline_history.iter().copied().fold(
            None,
            |lowest: Option<f64>, sample| {
                Some(lowest.map_or(sample, |lowest| lowest.min(sample)))
            },
        );
    }

    fn observe_rate(&mut self, now: Instant) -> Option<u64> {
        let elapsed = now.saturating_duration_since(self.window_started);
        if elapsed < RATE_WINDOW_MIN {
            return None;
        }
        let rate = (self.window_bytes as f64 / elapsed.as_secs_f64()) as u64;
        self.observed_rate = Some(rate);
        self.window_started = now;
        self.window_bytes = 0;
        Some(rate)
    }

    fn enter_ladder(&mut self, now: Instant) {
        if self.ladder_step as usize >= HOST_CONCURRENCY_COOLDOWN_LADDER.len() {
            // The ladder is exhausted: this authority is unusable for the rest
            // of the session, and pretending otherwise risks the measured
            // twenty-minute TCP blackhole.
            self.blocked = true;
            self.cooldown_until = None;
            return;
        }
        let step = usize::from(self.ladder_step);
        self.cooldown_until =
            Some(now + HOST_CONCURRENCY_COOLDOWN_LADDER[step]);
        self.ladder_step = self.ladder_step.saturating_add(1);
        self.plateau_windows = 0;
        self.last_rate = None;
    }
}

/// One adaptive limit per `(scheme, host, port)` and proxy scope.
pub(crate) struct HostPolicy {
    state: Mutex<HostState>,
    notify: Notify,
}

/// A snapshot of one host's admission state, for diagnostics and tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HostSnapshot {
    pub(crate) limit: usize,
    pub(crate) in_flight: usize,
    pub(crate) parked: bool,
    pub(crate) blocked: bool,
    pub(crate) observed_rate: Option<u64>,
}

impl HostPolicy {
    fn new(now: Instant) -> Self {
        Self {
            state: Mutex::new(HostState::new(now)),
            notify: Notify::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> HostSnapshot {
        let now = Instant::now();
        let state = self.state.lock();
        HostSnapshot {
            limit: state.limit,
            in_flight: state.in_flight,
            parked: state.parked(now),
            blocked: state.blocked,
            observed_rate: state.observed_rate,
        }
    }

    pub(crate) fn headroom(&self) -> usize {
        let now = Instant::now();
        let state = self.state.lock();
        if state.parked(now) {
            return 0;
        }
        state.limit.saturating_sub(state.in_flight)
    }

    /// Stream slots this host may carry on the multiplexed path.
    ///
    /// The learned limit counts *sockets*, and a stream is not a socket: the
    /// measurement is that sixteen streams on one connection tie eight parallel
    /// TCP connections, so one socket is worth about two streams. The stream
    /// allowance is therefore the learned socket limit scaled by that measured
    /// ratio, capped at [`H2_STREAMS_PER_HOST_CAP`] — the measured batch knee,
    /// not one connection's saturation point. Capping at the per-connection
    /// number made a single socket the binding constraint on a workload whose
    /// rate was still rising at 64 file-equivalents.
    fn stream_allowance_locked(state: &HostState) -> usize {
        state
            .limit
            .saturating_mul(H2_STREAMS_PER_LEARNED_SOCKET)
            .min(H2_STREAMS_PER_HOST_CAP)
    }

    /// Reserves one multiplexed stream against this host.
    fn reserve_stream(&self) -> Option<()> {
        let now = Instant::now();
        let mut state = self.state.lock();
        if state.parked(now) {
            return None;
        }
        if state.stream_in_flight >= Self::stream_allowance_locked(&state) {
            return None;
        }
        state.stream_in_flight += 1;
        Some(())
    }

    fn release_stream(&self, count: usize) {
        {
            let mut state = self.state.lock();
            state.stream_in_flight =
                state.stream_in_flight.saturating_sub(count);
        }
        self.notify.notify_waiters();
    }

    /// Waits for one multiplexed stream slot.
    async fn admit_stream(&self, wait: Duration) -> Result<(), AdmissionError> {
        let deadline = Instant::now() + wait;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.reserve_stream().is_some() {
                return Ok(());
            }
            {
                let now = Instant::now();
                let state = self.state.lock();
                if state.blocked {
                    return Err(AdmissionError::Blocked);
                }
                if let Some(until) = state.cooldown_until
                    && until > now
                {
                    return Err(AdmissionError::Cooling(until - now));
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdmissionError::Exhausted);
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Free multiplexed stream slots on this host right now.
    #[cfg(test)]
    pub(crate) fn stream_headroom(&self) -> usize {
        let now = Instant::now();
        let state = self.state.lock();
        if state.parked(now) {
            return 0;
        }
        Self::stream_allowance_locked(&state)
            .saturating_sub(state.stream_in_flight)
    }

    fn reserve(&self, size: SizeClass, host_cap: usize) -> Option<()> {
        let now = Instant::now();
        let mut state = self.state.lock();
        if state.parked(now) {
            return None;
        }
        if state.in_flight >= state.effective_limit(size, host_cap) {
            return None;
        }
        state.in_flight += 1;
        Some(())
    }

    fn release(&self, count: usize) {
        {
            let mut state = self.state.lock();
            state.in_flight = state.in_flight.saturating_sub(count);
        }
        self.notify.notify_waiters();
    }

    /// Waits for a slot. Bounded: a parked host reports itself instead of
    /// making the caller wait a whole cooldown for a transfer that has an
    /// official alternative.
    async fn admit(
        self: &Arc<Self>,
        size: SizeClass,
        host_cap: usize,
        wait: Duration,
    ) -> Result<(), AdmissionError> {
        let deadline = Instant::now() + wait;
        loop {
            // Register the wake-up *before* looking at the state. `notify_waiters`
            // only wakes futures that are already registered, so creating this
            // after the check would lose a release that happened in between and
            // make the caller wait out the whole timeout.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.reserve(size, host_cap).is_some() {
                return Ok(());
            }
            {
                let now = Instant::now();
                let state = self.state.lock();
                if state.blocked {
                    return Err(AdmissionError::Blocked);
                }
                if let Some(until) = state.cooldown_until
                    && until > now
                {
                    return Err(AdmissionError::Cooling(until - now));
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdmissionError::Exhausted);
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Adds delivered bytes to this host's measurement window.
    pub(crate) fn record_bytes(&self, bytes: u64) {
        let mut state = self.state.lock();
        state.window_bytes = state.window_bytes.saturating_add(bytes);
    }

    /// Records one finished request and applies the growth rules for its size.
    ///
    /// `_bytes` is the transfer's delivered total, kept for call-site symmetry
    /// and diagnostics only; it is deliberately *not* added to `window_bytes`.
    /// The per-chunk meter ([`TransferMeter::record`]) already accounts for
    /// every byte that arrived, so adding the total again here doubled the
    /// observed rate — a plateau (or a spurious improvement) computed from
    /// twice the real throughput.
    pub(crate) fn record_completion(
        &self,
        size: SizeClass,
        host_cap: usize,
        latency_ms: f64,
        _bytes: u64,
    ) {
        let now = Instant::now();
        let mut state = self.state.lock();
        if state.parked(now) {
            return;
        }
        state.completions = state.completions.saturating_add(1);
        state.push_latency(latency_ms);
        if state
            .last_strike_at
            .is_some_and(|at| now.saturating_duration_since(at) > STRIKE_DECAY)
        {
            state.strikes = 0;
        }
        if state.completions % HOST_CONCURRENCY_GROWTH_PER_COMPLETIONS != 0 {
            return;
        }
        let rate = state.observe_rate(now);
        if size == SizeClass::Bulk
            && let Some(rate) = rate
        {
            let plateau = state.last_rate.is_some_and(|previous| {
                u128::from(rate) * HOST_CONCURRENCY_PERCENT_BASE as u128
                    <= u128::from(previous) * PLATEAU_GAIN_PERCENT
            });
            state.plateau_windows = if plateau {
                state.plateau_windows.saturating_add(1)
            } else {
                0
            };
            state.last_rate = Some(rate);
        }
        // App-limited guard: with fewer than half the allowed transfers in
        // flight the limit is not what bounds throughput, so a latency change
        // is self-inflicted queueing rather than congestion.
        if state.in_flight * 2 < state.effective_limit(size, host_cap) {
            return;
        }
        if !state.latency_within_baseline() {
            return;
        }
        if state.plateau_windows >= PLATEAU_WINDOWS {
            return;
        }
        let ceiling = size.ceiling(host_cap);
        if state.limit >= ceiling {
            return;
        }
        let step = if size != SizeClass::Small
            && state.limit < HOST_CONCURRENCY_FAST_GROWTH_CEILING
        {
            HOST_CONCURRENCY_FAST_GROWTH_STEP
        } else {
            HOST_CONCURRENCY_GROWTH_STEP
        };
        state.limit = (state.limit + step).min(ceiling);
        state.completions = 0;
        drop(state);
        self.notify.notify_waiters();
    }

    /// Reset/refused/timeout: the measured fast-attack response is ×0.7.
    ///
    /// This is deliberately *not* called for `429`: on BMCLAPI a `429`
    /// describes the redirector's metadata budget, and treating it as socket
    /// capacity would halve a node that is serving bytes perfectly well.
    pub(crate) fn record_fast_failure(&self) {
        let now = Instant::now();
        let mut state = self.state.lock();
        state.limit = (state.limit * HOST_CONCURRENCY_FAST_ATTACK_PERCENT
            / HOST_CONCURRENCY_PERCENT_BASE)
            .max(HOST_CONCURRENCY_FLOOR);
        state.completions = 0;
        state.plateau_windows = 0;
        state.last_rate = None;
        state.strikes = state.strikes.saturating_add(1);
        state.last_strike_at = Some(now);
        let escalate = state.strikes >= STRIKES_BEFORE_LADDER;
        if escalate {
            state.strikes = 0;
            state.enter_ladder(now);
        }
        drop(state);
        self.notify.notify_waiters();
    }

    /// Sustained overrun or a suspected ban: climb the cooldown ladder.
    pub(crate) fn record_suspected_ban(&self) {
        let now = Instant::now();
        let mut state = self.state.lock();
        state.enter_ladder(now);
        drop(state);
        self.notify.notify_waiters();
    }

    fn expire(&self, now: Instant) {
        let mut state = self.state.lock();
        if state.cooldown_until.is_some_and(|until| until <= now) {
            state.cooldown_until = None;
        }
    }
}

/// Admission held for the life of one transfer.
pub(crate) struct HostPermit {
    policy: Option<Arc<HostPolicy>>,
    _global: Option<OwnedSemaphorePermit>,
}

impl Drop for HostPermit {
    fn drop(&mut self) {
        if let Some(policy) = &self.policy {
            policy.release(1);
        }
    }
}

/// Why a host could not admit a transfer.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AdmissionError {
    /// The limit is saturated; callers may wait and retry.
    Exhausted,
    /// Cooling down after failures; the host is skipped for now.
    Cooling(Duration),
    /// Blocked for the rest of the session.
    Blocked,
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exhausted => write!(formatter, "host admission is saturated"),
            Self::Cooling(after) => write!(
                formatter,
                "host is cooling down for {}s after failures",
                after.as_secs()
            ),
            Self::Blocked => {
                write!(formatter, "host is blocked for this session")
            }
        }
    }
}

impl From<AdmissionError> for crate::ErrorKind {
    fn from(error: AdmissionError) -> Self {
        Self::NetworkError(error.to_string())
    }
}

/// The process-wide in-flight ceiling.
///
/// Sockets held by the HTTP/2 pool are *not* counted here: a long-lived
/// multiplexed socket is not an in-flight byte transfer, and charging it twice
/// would starve the transfers it serves.
static GLOBAL_IN_FLIGHT: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(GLOBAL_IN_FLIGHT_CAP)));

static POLICIES: LazyLock<Mutex<BoundedTable<String, Arc<HostPolicy>>>> =
    LazyLock::new(|| Mutex::new(BoundedTable::new(POLICY_TABLE_CAPACITY)));

/// `(scheme, host, port)` identity of one post-redirect hop.
///
/// The scheme is part of the key because `http` and `https` to the same name
/// are different servers in practice, and the port because a mirror on `:8443`
/// is not the same authority as the one on `:443`.
pub(crate) fn hop_identity(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let port = url.port_or_known_default()?;
    Some(format!("{}://{host}:{port}", url.scheme()))
}

/// The host name of a URL, for the measured-ceiling table.
fn hop_host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
}

const POLICY_TABLE_CAPACITY: usize = 256;

/// Whether a policy may be dropped from the table.
///
/// Two things make an entry unrecoverable if it is forgotten:
///
/// * a live permit still holds an `Arc` (`strong_count > 1`), and dropping the
///   entry would let the next lookup create a second policy for the same scope —
///   both would then admit a full limit on top of each other; and
/// * the host is parked — cooling down or `blocked` for the session. A forgotten
///   ban is the expensive case: `policy_for_scope` would recreate the host
///   healthy and the measured ~20-minute TCP blackhole would be rediscovered by
///   the next request.
fn policy_is_droppable(policy: &Arc<HostPolicy>) -> bool {
    Arc::strong_count(policy) == 1
        && !policy.state.lock().parked(Instant::now())
}

/// The policy for one proxy-scoped authority, created on first use.
///
/// The table is bounded by [`POLICY_TABLE_CAPACITY`], and the bound is enforced
/// on insertion rather than by pruning alone: a `retain(strong_count > 1)` pass
/// frees nothing when every entry is in use, which is exactly the case a wide
/// install produces, so `len` could otherwise grow without limit.
///
/// Eviction order is least-recently-used, and it never takes an entry that is
/// still referenced by a permit or one that is parked — see
/// [`policy_is_droppable`].
pub(crate) fn policy_for_scope(scope: &str) -> Arc<HostPolicy> {
    let now = Instant::now();
    let mut policies = POLICIES.lock();
    policies.get_or_insert_with(
        scope.to_ascii_lowercase(),
        || Arc::new(HostPolicy::new(now)),
        policy_is_droppable,
    )
}

/// Hard per-host ceilings from the measurement table.
///
/// These are not guesses about a generic server: they are the measured knee of
/// one authority each, and they bound the learned limit rather than replacing
/// it. A host with no measurement keeps
/// [`HOST_CONCURRENCY_CAP_UNMEASURED`], which is the largest socket plan the
/// per-authority socket budget can fund.
pub(crate) fn host_cap(host: &str) -> usize {
    let host = host.to_ascii_lowercase();
    if host.contains("aliyun") {
        // Knee 8–16; 13.95 MB/s at 8 and 9.4 MB/s at 64.
        return 16;
    }
    if host.contains("ustc.edu.cn") {
        // ConnectionRefused from 16 up; 731 RSTs at N=96.
        return HOST_CONCURRENCY_CAP_MEASURED_MIN;
    }
    if host == "libraries.minecraft.net" {
        // 91.8 MB/s at 64 and zero errors to 96.
        return 64;
    }
    HOST_CONCURRENCY_CAP_UNMEASURED
}

/// Whether a host has been measured to honour `Range` at all.
///
/// `cmcc.mirrors.ustc.edu.cn` answers a ranged request with `200` and the whole
/// body, so a split there does not merely fail: it multiplies one file's bytes
/// by the number of range workers. Range splitting must never be planned
/// against it, whatever the caller asks for.
pub(crate) fn honours_range_requests(host: &str) -> bool {
    !host.to_ascii_lowercase().contains("ustc.edu.cn")
}

/// The host that actually serves a route's bytes.
///
/// `DownloadRoute::url` names the requested host; a redirector answers every
/// file `GET` with a `302` to a rotating node, so admission must charge the
/// node. Before the first hop is learned, the requested host is the only name
/// available.
fn admission_url(route: &DownloadRoute) -> String {
    super::route_health::effective_route_authority(route)
        .map(|authority| format!("https://{authority}"))
        .unwrap_or_else(|| route.url.clone())
}

fn policy_for_url(url: &str, proxy: ProxyPolicy) -> Option<Arc<HostPolicy>> {
    let identity = hop_identity(url)?;
    Some(policy_for_scope(&super::proxy_context::authority_key(
        &identity, proxy,
    )))
}

/// `host:port` of the host a route's sockets land on, after redirects.
pub(crate) fn admission_authority(route: &DownloadRoute) -> Option<String> {
    let url = admission_url(route);
    let parsed = url::Url::parse(&url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default()?;
    Some(format!("{host}:{port}"))
}

/// Proxy-scoped identity of the host a route's connections land on, in the
/// `host:port|scope` form the socket budgets already use.
pub(crate) fn scope_for_route(route: &DownloadRoute) -> Option<String> {
    Some(super::proxy_context::authority_key(
        &admission_authority(route)?,
        route.proxy,
    ))
}

/// Pool-facing scope of the *requested* host.
///
/// The HTTP/2 pool indexes its slots by the URL it was asked to connect to,
/// not by the URL a redirect eventually produced, so an eviction has to use
/// this key to find the slot it means. Admission uses [`scope_for_route`].
pub(crate) fn pool_scope_for_route(route: &DownloadRoute) -> Option<String> {
    Some(super::proxy_context::authority_key(
        &crate::util::fetch::url_authority(&route.url)?,
        route.proxy,
    ))
}

fn cap_for_url(url: &str) -> usize {
    hop_host(url)
        .map_or(HOST_CONCURRENCY_CAP_UNMEASURED, |host| host_cap(&host))
}

/// Admission and release for one transfer against its effective host.
///
/// # Counter balance
///
/// `in_flight` is incremented exactly once per [`HostPermit`] handed out, and
/// [`HostPermit::drop`] decrements it exactly once. Every path out of this
/// function therefore either returns a permit — the only object that can
/// release a reservation — or holds no reservation:
///
/// * The host admit fails: `reserve` made no increment, nothing to undo.
/// * The host admit succeeds but the global ceiling is full: the host slot is
///   released in the same synchronous block, so the reservation and its release
///   are adjacent. The global permit that is then awaited is a local
///   `OwnedSemaphorePermit`; if this future is dropped during that wait, or the
///   re-admit below fails, the local's `Drop` returns it and no host slot is
///   held.
/// * The re-admit fails: the freshly acquired global permit is dropped on the
///   error return; the host slot was never taken.
/// * Success: `HostPermit` holds both, and the single `Drop` impl releases
///   both. There is no `await` between a successful host admit and either the
///   permit construction or the matching `release`, so a cancellation at a
///   suspension point can never strand a reservation.
///
/// This is why the host slot is re-taken after the global permit arrives rather
/// than reused from before the wait: the old shape released the slot, waited
/// for the ceiling, and then built the permit without reserving again, leaving
/// `in_flight` one short for the life of the host.
pub(crate) async fn acquire(
    route: &DownloadRoute,
    size: Option<u64>,
    wait: Duration,
) -> Result<HostPermit, AdmissionError> {
    let url = admission_url(route);
    let Some(policy) = policy_for_url(&url, route.proxy) else {
        return Ok(HostPermit {
            policy: None,
            _global: None,
        });
    };
    let class = SizeClass::of(size);
    let cap = cap_for_url(&url);
    let global = GLOBAL_IN_FLIGHT.clone();
    let deadline = Instant::now() + wait;
    policy.admit(class, cap, wait).await?;
    let permit = match global.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            // The process-wide ceiling is reached. Hand the host slot back
            // before waiting so a busy host cannot hold capacity it cannot
            // use, then re-take it once the ceiling admits this transfer.
            policy.release(1);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdmissionError::Exhausted);
            }
            let Ok(Ok(permit)) =
                tokio::time::timeout(remaining, global.clone().acquire_owned())
                    .await
            else {
                return Err(AdmissionError::Exhausted);
            };
            // Re-reserving is what balances the release above: the returned
            // `HostPermit` releases exactly this reservation on drop, so
            // `in_flight` moves -1 then +1 and ends where it started.
            let remaining = deadline.saturating_duration_since(Instant::now());
            policy.admit(class, cap, remaining).await?;
            permit
        }
    };
    Ok(HostPermit {
        policy: Some(policy),
        _global: Some(permit),
    })
}

/// Non-blocking admission for recovery and probe paths.
pub(crate) fn try_acquire(
    route: &DownloadRoute,
    size: Option<u64>,
) -> Option<HostPermit> {
    let url = admission_url(route);
    let policy = policy_for_url(&url, route.proxy)?;
    let global = GLOBAL_IN_FLIGHT.clone().try_acquire_owned().ok()?;
    policy.reserve(SizeClass::of(size), cap_for_url(&url))?;
    Some(HostPermit {
        policy: Some(policy),
        _global: Some(global),
    })
}

/// Admission for one multiplexed HTTP/2 stream.
///
/// A stream is a transfer's worth of work against the serving host, so it must
/// respect that host's learned limit: otherwise the controller never sees the
/// multiplexed path at all, and a host that kneed at four sockets is handed
/// thirty-two streams while its learned limit keeps reporting four.
///
/// The allowance is the learned limit scaled by the measured multiplexing ratio
/// (`H2_STREAMS_PER_LEARNED_SOCKET`) and capped at one connection's saturation
/// point (`H2_STREAMS_PER_CONNECTION_CAP`). Charging a stream as a whole socket
/// would give a fresh host four streams where four plain sockets already
/// deliver eight, so multiplexing could never pay for itself.
///
/// It deliberately does *not* take a [`GLOBAL_IN_FLIGHT_CAP`] permit. That
/// ceiling exists to bound large byte transfers; charging every asset stream
/// against it would make a four-hundred-file asset batch compete with the
/// downloads the cap protects, and the batch already bounds its own logical
/// workers through the configured download concurrency. The socket those
/// streams share is charged separately by [`super::native_budget`].
pub(crate) async fn acquire_stream(
    route: &DownloadRoute,
    wait: Duration,
) -> Result<HostStreamPermit, AdmissionError> {
    let url = admission_url(route);
    let Some(policy) = policy_for_url(&url, route.proxy) else {
        return Ok(HostStreamPermit { policy: None });
    };
    policy.admit_stream(wait).await?;
    Ok(HostStreamPermit {
        policy: Some(policy),
    })
}

/// One stream's slot in the serving host's learned multiplexed allowance.
pub(crate) struct HostStreamPermit {
    policy: Option<Arc<HostPolicy>>,
}

impl Drop for HostStreamPermit {
    fn drop(&mut self) {
        if let Some(policy) = &self.policy {
            policy.release_stream(1);
        }
    }
}

/// Free multiplexed stream slots on the host a route would land on.
#[cfg(test)]
pub(crate) fn stream_headroom(route: &DownloadRoute) -> usize {
    let url = admission_url(route);
    policy_for_url(&url, route.proxy)
        .map(|policy| policy.stream_headroom())
        .unwrap_or(H2_STREAMS_PER_CONNECTION_CAP)
}

/// Free slots on the host a route would land on.
pub(crate) fn available(route: &DownloadRoute) -> usize {
    let url = admission_url(route);
    policy_for_url(&url, route.proxy)
        .map(|policy| policy.headroom())
        .unwrap_or(HOST_CONCURRENCY_CAP_UNMEASURED)
}

/// Transfers currently in flight against that host.
pub(crate) fn in_use(route: &DownloadRoute) -> usize {
    let url = admission_url(route);
    policy_for_url(&url, route.proxy)
        .map(|policy| policy.snapshot().in_flight)
        .unwrap_or(0)
}

/// Growth ceiling for a file's connection count — the host's *measured* cap,
/// not its currently learned limit.
///
/// A range plan is drawn before any transfer has been measured, so reading the
/// learned limit here would cap every fresh host at four ranges and turn a
/// 200 MB archive into a four-way download. Admission still enforces the
/// learned limit per transfer; planning is deliberately optimistic and the
/// host's measured knee is the hard ceiling.
pub(crate) fn capacity(route: &DownloadRoute) -> usize {
    cap_for_url(&admission_url(route))
}

/// Records a finished transfer against the host that served it.
pub(crate) fn record_completion(
    final_url: &str,
    proxy: ProxyPolicy,
    size: Option<u64>,
    latency: Duration,
    bytes: u64,
) {
    let Some(identity) = hop_identity(final_url) else {
        return;
    };
    let policy = policy_for_scope(&super::proxy_context::authority_key(
        &identity, proxy,
    ));
    policy.record_completion(
        SizeClass::of(size),
        cap_for_url(final_url),
        latency.as_secs_f64() * 1000.0,
        bytes,
    );
}

/// Records a reset, refusal, timeout or `5xx` against the serving host.
pub(crate) fn record_fast_failure(final_url: &str, proxy: ProxyPolicy) {
    let Some(identity) = hop_identity(final_url) else {
        return;
    };
    policy_for_scope(&super::proxy_context::authority_key(&identity, proxy))
        .record_fast_failure();
}

/// Records a suspected ban (a burst of failures with no delivered bytes).
pub(crate) fn record_suspected_ban(final_url: &str, proxy: ProxyPolicy) {
    let Some(identity) = hop_identity(final_url) else {
        return;
    };
    policy_for_scope(&super::proxy_context::authority_key(&identity, proxy))
        .record_suspected_ban();
}

/// A precomputed attribution target for one in-flight transfer.
///
/// The proxy scope is a content hash of the proxy configuration, so it is
/// computed once per transfer and then reused: recomputing it for every
/// received chunk would hash the configuration thousands of times per file.
#[derive(Clone)]
pub(crate) struct TransferMeter {
    scope: String,
}

impl TransferMeter {
    pub(crate) fn for_url(url: &str, proxy: ProxyPolicy) -> Option<Self> {
        let identity = hop_identity(url)?;
        Some(Self {
            scope: super::proxy_context::authority_key(&identity, proxy),
        })
    }

    /// Adds delivered bytes to the host's measurement window.
    pub(crate) fn record(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        policy_for_scope(&self.scope).record_bytes(bytes);
    }
}

/// Observed rate for the host that served a transfer.
///
/// This is the "why is this slow?" answer: the UI can pair the active source
/// with the rate that source actually delivered, instead of comparing the
/// aggregate job speed against a number no host ever produced.
pub(crate) fn observed_rate(
    final_url: &str,
    proxy: ProxyPolicy,
) -> Option<u64> {
    let identity = hop_identity(final_url)?;
    policy_for_scope(&super::proxy_context::authority_key(&identity, proxy))
        .snapshot()
        .observed_rate
}

/// Expires elapsed cooldowns. Called from the existing maintenance tick so a
/// parked host recovers without waiting for another transfer to notice.
pub(crate) fn observe_all() {
    let now = Instant::now();
    let policies: Vec<_> = POLICIES.lock().values().cloned().collect();
    for policy in policies {
        policy.expire(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::download::constants::HOST_CONCURRENCY_CAP_BULK;
    use crate::util::fetch::DownloadRouteSource;

    fn route(host: &str) -> DownloadRoute {
        DownloadRoute {
            url: format!("https://{host}/file"),
            source: DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: ProxyPolicy::Direct,
        }
    }

    #[test]
    fn the_identity_is_scheme_host_and_port() {
        assert_eq!(
            hop_identity("https://Example.COM/file").as_deref(),
            Some("https://example.com:443")
        );
        assert_eq!(
            hop_identity("http://example.com:8443/file").as_deref(),
            Some("http://example.com:8443")
        );
        assert_ne!(
            hop_identity("https://example.com/file"),
            hop_identity("http://example.com/file")
        );
    }

    #[test]
    fn measured_hosts_keep_their_measured_ceilings() {
        assert_eq!(host_cap("maven.aliyun.com"), 16);
        assert_eq!(host_cap("cmcc.mirrors.ustc.edu.cn"), 8);
        assert_eq!(host_cap("libraries.minecraft.net"), 64);
        assert_eq!(
            host_cap("piston-data.mojang.com"),
            HOST_CONCURRENCY_CAP_UNMEASURED
        );
    }

    #[test]
    fn small_objects_never_earn_the_bulk_ceiling() {
        assert_eq!(SizeClass::of(Some(121 * 1024)), SizeClass::Small);
        assert_eq!(SizeClass::of(Some(2 * 1024 * 1024)), SizeClass::Bulk);
        assert_eq!(
            SizeClass::Small.ceiling(HOST_CONCURRENCY_CAP_BULK),
            SMALL_FILE_CONCURRENCY_CEILING
        );
        assert_eq!(SizeClass::Bulk.ceiling(HOST_CONCURRENCY_CAP_BULK), 128);
    }

    #[test]
    fn a_measured_host_that_ignores_range_is_never_split() {
        assert!(!honours_range_requests("cmcc.mirrors.ustc.edu.cn"));
        assert!(honours_range_requests("libraries.minecraft.net"));
    }

    #[test]
    fn every_host_starts_at_four_and_the_app_limited_guard_blocks_growth() {
        let policy = HostPolicy::new(Instant::now());
        assert_eq!(policy.snapshot().limit, HOST_CONCURRENCY_INITIAL);
        // Nothing in flight: the limit is not the bottleneck, so a latency
        // sample cannot justify growth.
        for _ in 0..32 {
            policy.record_completion(SizeClass::Bulk, 128, 40.0, 1 << 20);
        }
        assert_eq!(policy.snapshot().limit, HOST_CONCURRENCY_INITIAL);
    }

    #[test]
    fn growth_is_additive_and_stops_at_the_host_cap() {
        let policy = HostPolicy::new(Instant::now());
        let cap = 16;
        for _ in 0..cap {
            policy.reserve(SizeClass::Bulk, cap);
        }
        for _ in 0..512 {
            policy.record_completion(SizeClass::Bulk, cap, 40.0, 1 << 20);
        }
        assert!(policy.snapshot().limit > HOST_CONCURRENCY_INITIAL);
        assert!(policy.snapshot().limit <= cap);
    }

    #[test]
    fn a_fast_failure_applies_the_measured_seventy_percent_attack() {
        let policy = HostPolicy::new(Instant::now());
        policy.state.lock().limit = 64;
        policy.record_fast_failure();
        assert_eq!(policy.snapshot().limit, 44);
    }

    #[test]
    fn the_cooldown_ladder_escalates_to_a_session_block() {
        let policy = HostPolicy::new(Instant::now());
        for _ in 0..HOST_CONCURRENCY_COOLDOWN_LADDER.len() {
            policy.record_suspected_ban();
            assert!(policy.snapshot().parked);
            policy.expire(Instant::now() + Duration::from_secs(2400));
        }
        policy.record_suspected_ban();
        assert!(policy.snapshot().blocked);
        assert_eq!(policy.headroom(), 0);
    }

    #[tokio::test]
    async fn a_parked_host_reports_itself_instead_of_waiting() {
        let route = route("parked-admission.example");
        let policy = policy_for_url(&route.url, route.proxy).unwrap();
        policy.record_suspected_ban();
        assert!(matches!(
            acquire(&route, None, Duration::from_millis(50)).await,
            Err(AdmissionError::Cooling(_))
        ));
    }
}
