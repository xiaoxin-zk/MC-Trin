//! Route health state, authority memory, and probe bookkeeping.

use super::route_policy;
use crate::util::download::constants::{
    DIAGNOSTICS_OTHER, ROUTE_HEALTH_CURSEFORGE, ROUTE_HEALTH_LOADER,
    ROUTE_HEALTH_MINECRAFT, ROUTE_HEALTH_MODRINTH, ROUTE_OBJECT_MISS_TTL,
    ROUTE_THROUGHPUT_PREFERENCE_PERCENT, ROUTE_THROUGHPUT_TRUST_SAMPLES,
    ROUTE_THROUGHPUT_TRUST_WINDOW,
};
use crate::util::fetch::{DownloadRoute, ProxyPolicy, ResourceClass};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResourceFamily {
    Minecraft,
    Loader,
    Modrinth,
    CurseForge,
    Other,
}
impl ResourceFamily {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Minecraft => ROUTE_HEALTH_MINECRAFT,
            Self::Loader => ROUTE_HEALTH_LOADER,
            Self::Modrinth => ROUTE_HEALTH_MODRINTH,
            Self::CurseForge => ROUTE_HEALTH_CURSEFORGE,
            Self::Other => DIAGNOSTICS_OTHER,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RouteHealthKey {
    pub(crate) family: ResourceFamily,
    pub(crate) authority: String,
    pub(crate) scope: String,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct RouteHealth {
    pub(crate) success_samples: u32,
    pub(crate) ttfb_ms: Option<f64>,
    pub(crate) throughput_bps: Option<f64>,
    /// Successful transfers behind `throughput_bps`.
    ///
    /// Separate from `success_samples` so a throughput ordering can require
    /// more than one sample without also requiring that many *metadata*
    /// answers: a manifest fetch also proves the route works, but it is not a
    /// throughput measurement of a file transfer.
    pub(crate) throughput_samples: u32,
    /// When `throughput_bps` was last updated, for the trust window.
    pub(crate) throughput_measured_at: Option<Instant>,
    pub(crate) consecutive_failures: u32,
    pub(crate) cooldown_until: Option<Instant>,
}

/// The rate this route may be *ordered* by, if it has one worth trusting.
///
/// Both conditions matter:
///
/// * the sample count, because one finished transfer is one object on one
///   connection and says little about a route; and
/// * the age, because a throughput number describes a network path at a
///   moment in time. A measurement older than `ROUTE_THROUGHPUT_TRUST_WINDOW`
///   is treated exactly like no measurement: ordering falls back to the user's
///   source preference instead of pinning a route on a number that may describe
///   a different network entirely.
///
/// That is also how the "never measured" route is protected: a stale value is
/// not a value here, so an unmeasured route is never sorted below a route whose
/// only claim is an old fast sample.
pub(crate) fn trusted_throughput(health: &RouteHealth) -> Option<f64> {
    let rate = health
        .throughput_bps
        .filter(|rate| rate.is_finite() && *rate > 0.0)?;
    if health.throughput_samples < ROUTE_THROUGHPUT_TRUST_SAMPLES {
        return None;
    }
    let measured_at = health.throughput_measured_at?;
    (measured_at.elapsed() <= ROUTE_THROUGHPUT_TRUST_WINDOW).then_some(rate)
}

/// Quantises a trusted rate into `ROUTE_THROUGHPUT_PREFERENCE_PERCENT`-wide
/// geometric buckets.
///
/// The bucket exists so the "is it really faster?" margin can be part of a
/// per-route key instead of a pairwise comparison. A pairwise margin is not
/// transitive (A may clear B's margin, B clear C's, and C still clear A's), and
/// a non-transitive comparator makes a sort's output depend on input order.
/// With buckets, any route at least 25 % faster necessarily lands in a strictly
/// higher bucket, so the margin is honoured and the ordering stays a key.
pub(crate) fn throughput_rank(rate: f64) -> i64 {
    (rate.ln() / ROUTE_THROUGHPUT_PREFERENCE_PERCENT.ln()).floor() as i64
}
pub(crate) static ROUTE_HEALTH: LazyLock<
    Mutex<HashMap<RouteHealthKey, RouteHealth>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) struct TaskProbeGuard {
    pub(crate) state: Arc<TaskProbeState>,
    pub(crate) family: ResourceFamily,
    pub(crate) notify: Arc<Notify>,
    pub(crate) armed: bool,
}
impl TaskProbeGuard {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for TaskProbeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut families = self.state.families.lock();
        if let Some(entry) = families.get_mut(&self.family)
            && entry
                .in_flight
                .as_ref()
                .is_some_and(|v| Arc::ptr_eq(v, &self.notify))
        {
            entry.in_flight = None;
            entry.last_probed = None;
        }
    }
}
pub(crate) static ROUTE_EFFECTIVE_AUTHORITIES: LazyLock<
    Mutex<HashMap<String, String>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TaskProbeKey {
    Job(Uuid, u64),
    Anonymous(u64),
}
#[derive(Default)]
pub(crate) struct TaskProbeState {
    pub(crate) families: Mutex<HashMap<ResourceFamily, FamilyProbeState>>,
}
impl TaskProbeState {
    pub(crate) fn has_in_flight(&self) -> bool {
        self.families.lock().values().any(|f| f.in_flight.is_some())
    }
}
#[derive(Default)]
pub(crate) struct FamilyProbeState {
    pub(crate) last_probed: Option<Instant>,
    pub(crate) in_flight: Option<Arc<Notify>>,
}
pub(crate) static TASK_PROBE_STATES: LazyLock<
    Mutex<HashMap<TaskProbeKey, Arc<TaskProbeState>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn resource_family(
    route: &DownloadRoute,
    resource: ResourceClass,
) -> ResourceFamily {
    match resource {
        ResourceClass::MinecraftLibrary
            if route_policy::is_loader_repository(&route.url, resource) =>
        {
            ResourceFamily::Loader
        }
        ResourceClass::Metadata
        | ResourceClass::MinecraftAsset
        | ResourceClass::MinecraftLibrary
        | ResourceClass::Java => ResourceFamily::Minecraft,
        ResourceClass::Loader => ResourceFamily::Loader,
        ResourceClass::Modrinth | ResourceClass::Modpack => {
            ResourceFamily::Modrinth
        }
        ResourceClass::CurseForge => ResourceFamily::CurseForge,
        ResourceClass::Other => ResourceFamily::Other,
    }
}
pub(crate) fn route_health_key(
    route: &DownloadRoute,
    resource: ResourceClass,
) -> Option<RouteHealthKey> {
    Some(RouteHealthKey {
        family: resource_family(route, resource),
        authority: crate::util::fetch::range_splitting_authority(route)?,
        scope: super::proxy_context::scope(route.proxy),
    })
}
pub(crate) fn persisted_route_health(
    key: &RouteHealthKey,
    proxy: ProxyPolicy,
) -> RouteHealth {
    crate::util::download::native_reputation::get(
        key.family.as_str(),
        &key.authority,
        proxy,
    )
    .map(|p| RouteHealth {
        success_samples: p.success_samples,
        ttfb_ms: p.ttfb_ms,
        throughput_bps: p.throughput_bps,
        // The persisted record keeps one success counter and one EWMA rate, so
        // the number of successes is the only bound available on how many
        // samples are behind the rate. That is conservative in the right
        // direction: a route needs at least this many successes before its
        // persisted rate may reorder anything.
        throughput_samples: p.success_samples,
        throughput_measured_at: p.throughput_measured_at(),
        consecutive_failures: p.consecutive_failures,
        cooldown_until: None,
    })
    .unwrap_or_default()
}
pub(crate) fn effective_route_authority(
    route: &DownloadRoute,
) -> Option<String> {
    let authority = crate::util::fetch::url_authority(&route.url)?;
    ROUTE_EFFECTIVE_AUTHORITIES
        .lock()
        .get(&super::proxy_context::authority_key(
            &route.url,
            route.proxy,
        ))
        .cloned()
        .or(Some(authority))
}
pub(crate) fn remember_effective_route_authority(
    route: &DownloadRoute,
    final_url: &str,
) {
    let (Some(original), Some(effective)) = (
        crate::util::fetch::url_authority(&route.url),
        crate::util::fetch::url_authority(final_url),
    ) else {
        return;
    };
    let mut map = ROUTE_EFFECTIVE_AUTHORITIES.lock();
    if original == effective {
        map.remove(&super::proxy_context::authority_key(
            &route.url,
            route.proxy,
        ));
    } else {
        map.insert(
            super::proxy_context::authority_key(&route.url, route.proxy),
            effective,
        );
    }
}
pub(crate) fn forget_effective_route_authority(
    route: &DownloadRoute,
    failed_url: &url::Url,
) {
    let Some(failed) = crate::util::fetch::url_authority(failed_url.as_str())
    else {
        return;
    };
    let mut map = ROUTE_EFFECTIVE_AUTHORITIES.lock();
    if map.get(&super::proxy_context::authority_key(
        &route.url,
        route.proxy,
    )) == Some(&failed)
    {
        map.remove(&super::proxy_context::authority_key(
            &route.url,
            route.proxy,
        ));
    }
}

use super::constants::ROUTE_HEALTH_EWMA_ALPHA as EWMA_ALPHA;

/// Updates a metric using the same weighted moving average for every route.
pub(crate) fn update_ewma(current: &mut Option<f64>, sample: f64) {
    *current = Some(current.map_or(sample, |current| {
        current * (1.0 - EWMA_ALPHA) + sample * EWMA_ALPHA
    }));
}

/// Stable scope identifier for task-level route probes.
pub(crate) fn probe_scope(family: &str, authorities: &mut Vec<String>) -> u64 {
    use std::hash::{Hash, Hasher};
    authorities.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    family.hash(&mut hasher);
    authorities.hash(&mut hasher);
    hasher.finish()
}

/// Paths a route has already answered `404`/`410` for.
///
/// A miss is a property of one object on one route, never of the route: USTC
/// answers `404` for BMCLAPI's version API, and 13 % of small Maven paths are
/// absent from `maven.aliyun.com/repository/public`. Both are statements that
/// *this* route serves no artifact for *this* path, so the route is not offered
/// for that path again and the authority is not demoted for every other object
/// it does serve.
///
/// Bounded and expiring on purpose: a mirror can synchronise a missing object
/// later, so an entry lives for [`ROUTE_OBJECT_MISS_TTL`] rather than forever.
/// It *is* written to disk, because a per-session memo only saves the second
/// install of a session: with a persisted memo the very first install of a
/// fresh launcher no longer pays one guaranteed `404` per unmirrored classifier
/// path. That is also why no HEAD preflight exists — a HEAD costs a full RTT
/// for every path, including the 87 % that resolve, while a memo costs nothing
/// for the ones already known.
static OBJECT_MISSES: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const OBJECT_MISS_MEMORY_CAPACITY: usize = 8192;

/// Whether the persisted memo has been merged into this session already.
static OBJECT_MISSES_LOADED: tokio::sync::OnceCell<()> =
    tokio::sync::OnceCell::const_new();
static OBJECT_MISS_WRITE_SCHEDULED: AtomicBool = AtomicBool::new(false);
/// Bumped by every recorded miss so a write in flight knows it is stale.
static OBJECT_MISS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// File name under the settings directory.
const OBJECT_MISS_FILE_NAME: &str = "download_object_misses.json";
const OBJECT_MISS_WRITE_DELAY: Duration = Duration::from_secs(5);

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedObjectMisses {
    version: u32,
    /// `authority + path` keys with the unix second they were learned at.
    misses: Vec<(String, u64)>,
}

const OBJECT_MISS_SCHEMA_VERSION: u32 = 1;

fn object_miss_key(route_url: &str) -> Option<String> {
    crate::util::fetch::url_authority(route_url)
        .map(|authority| format!("{authority}{}", url_path_of(route_url)))
}

fn url_path_of(url: &str) -> String {
    url::Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_default()
}

fn object_miss_path() -> Option<std::path::PathBuf> {
    crate::State::get_if_initialized()
        .map(|state| state.directories.settings_dir.join(OBJECT_MISS_FILE_NAME))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Drops entries that outlived [`ROUTE_OBJECT_MISS_TTL`].
fn prune_object_misses(misses: &mut HashMap<String, Instant>) {
    if misses
        .values()
        .all(|learned_at| learned_at.elapsed() <= ROUTE_OBJECT_MISS_TTL)
    {
        return;
    }
    misses
        .retain(|_, learned_at| learned_at.elapsed() <= ROUTE_OBJECT_MISS_TTL);
}

/// Merges the persisted memo into this session, once.
///
/// Called from the download path rather than from state initialization so a
/// session that never downloads anything never touches the file. A missing or
/// unreadable file is not an error: the memo is an optimisation, and
/// re-learning a miss costs one request.
pub(crate) async fn ensure_object_misses_loaded() {
    if OBJECT_MISSES_LOADED.get().is_some() {
        return;
    }
    // Resolve the path before marking the load as done: a download that starts
    // before `State` is initialized would otherwise latch "loaded" with nothing
    // read and never pick the file up again.
    let Some(path) = object_miss_path() else {
        return;
    };
    OBJECT_MISSES_LOADED
        .get_or_init(|| async move { load_object_misses(&path).await })
        .await;
}

async fn load_object_misses(path: &std::path::Path) {
    let Ok(contents) = tokio::fs::read(path).await else {
        return;
    };
    let Ok(store) = serde_json::from_slice::<PersistedObjectMisses>(&contents)
    else {
        tracing::debug!(
            path = %path.display(),
            "Ignoring invalid download object-miss memo"
        );
        return;
    };
    if store.version != OBJECT_MISS_SCHEMA_VERSION {
        return;
    }
    let now = unix_now();
    let ttl = ROUTE_OBJECT_MISS_TTL.as_secs();
    let mut misses = OBJECT_MISSES.lock();
    for (key, learned_at) in store.misses {
        let Some(age) = now.checked_sub(learned_at) else {
            continue;
        };
        if age > ttl {
            continue;
        }
        // An age the monotonic clock cannot represent means this process has
        // not been up that long; treat it as expired rather than pretending a
        // stale miss is fresh.
        let Some(instant) =
            Instant::now().checked_sub(std::time::Duration::from_secs(age))
        else {
            continue;
        };
        if misses.len() >= OBJECT_MISS_MEMORY_CAPACITY {
            break;
        }
        misses.insert(key, instant);
    }
}

fn schedule_object_miss_write() {
    // Bump before anything else so a miss recorded while the write is in flight
    // is never lost: the writer compares the generation it captured with the
    // current one and loops when they differ.
    OBJECT_MISS_GENERATION.fetch_add(1, Ordering::AcqRel);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if OBJECT_MISS_WRITE_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    runtime.spawn(async {
        loop {
            let generation = OBJECT_MISS_GENERATION.load(Ordering::Acquire);
            tokio::time::sleep(OBJECT_MISS_WRITE_DELAY).await;
            if let Err(error) = write_object_misses().await {
                tracing::debug!(
                    %error,
                    "Failed to persist download object-miss memo"
                );
            }
            if OBJECT_MISS_GENERATION.load(Ordering::Acquire) == generation {
                OBJECT_MISS_WRITE_SCHEDULED.store(false, Ordering::Release);
                if OBJECT_MISS_GENERATION.load(Ordering::Acquire) == generation
                    || OBJECT_MISS_WRITE_SCHEDULED.swap(true, Ordering::AcqRel)
                {
                    break;
                }
            }
        }
    });
}

async fn write_object_misses() -> std::io::Result<()> {
    let Some(path) = object_miss_path() else {
        return Ok(());
    };
    let now = unix_now();
    let mut entries = {
        // Keep the guard inside its own scope so it can never be captured by
        // the future across an await below: `parking_lot`'s guard is not
        // `Send`, and this function is spawned as a task.
        let mut misses = OBJECT_MISSES.lock();
        prune_object_misses(&mut misses);
        misses
            .iter()
            .map(|(key, learned_at)| {
                // The in-memory value is monotonic; store its age against wall
                // clock so a restart can still measure how old it is.
                (
                    key.clone(),
                    now.saturating_sub(learned_at.elapsed().as_secs()),
                )
            })
            .collect::<Vec<_>>()
    };
    // Newest first, so the truncation below drops the oldest learnings rather
    // than the freshest ones.
    entries.sort_unstable_by(|left, right| right.1.cmp(&left.1));
    entries.truncate(OBJECT_MISS_MEMORY_CAPACITY);
    let bytes = serde_json::to_vec(&PersistedObjectMisses {
        version: OBJECT_MISS_SCHEMA_VERSION,
        misses: entries,
    })
    .map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension("json.tmp");
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error);
        }
        tokio::fs::remove_file(&path).await?;
        tokio::fs::rename(&temporary, &path).await?;
    }
    Ok(())
}

/// `404`/`410` from this route for this object.
pub(crate) fn remember_object_miss(route_url: &str) {
    let Some(key) = object_miss_key(route_url) else {
        return;
    };
    let mut misses = OBJECT_MISSES.lock();
    if misses.len() >= OBJECT_MISS_MEMORY_CAPACITY {
        prune_object_misses(&mut misses);
        if misses.len() >= OBJECT_MISS_MEMORY_CAPACITY {
            // Evict by age rather than clearing: the newest learnings are the
            // ones the current install will ask about again, so dropping the
            // whole memo would re-pay a guaranteed `404` for every path it just
            // learned.
            let excess = misses.len() + 1 - OBJECT_MISS_MEMORY_CAPACITY;
            let mut oldest: Vec<(String, Instant)> = misses
                .iter()
                .map(|(key, learned_at)| (key.clone(), *learned_at))
                .collect();
            // `Instant` is monotonic and only comparable, so sort oldest first.
            oldest.sort_unstable_by_key(|(_, learned_at)| *learned_at);
            for (key, _) in oldest.into_iter().take(excess) {
                misses.remove(&key);
            }
        }
    }
    misses.insert(key, Instant::now());
    drop(misses);
    schedule_object_miss_write();
}

/// Whether this route already said it has no artifact for this path.
#[cfg(test)]
pub(crate) fn object_miss_is_known(route_url: &str) -> bool {
    object_miss_key(route_url).is_some_and(|key| {
        let mut misses = OBJECT_MISSES.lock();
        let Some(learned_at) = misses.get(&key).copied() else {
            return false;
        };
        if learned_at.elapsed() > ROUTE_OBJECT_MISS_TTL {
            misses.remove(&key);
            return false;
        }
        true
    })
}

/// Drops routes that already reported this exact object missing.
///
/// Runs after ordering, so it can only remove a route the caller had decided to
/// try. It never empties the plan: the memo is an optimisation, and a plan with
/// no route at all turns a recoverable `404` into a hard failure.
pub(crate) fn drop_known_object_misses(routes: &mut Vec<DownloadRoute>) {
    if routes.len() <= 1 {
        return;
    }
    // Read the live set under one lock rather than asking per route: the memo
    // is shared, and one snapshot keeps the decision consistent across the
    // plan.
    let live = {
        let mut misses = OBJECT_MISSES.lock();
        if misses.is_empty() {
            return;
        }
        prune_object_misses(&mut misses);
        misses.keys().cloned().collect::<HashSet<_>>()
    };
    if live.is_empty() {
        return;
    }
    // Keep exactly the routes that have not answered `404`/`410` for this
    // path. If that would empty the plan — the memo is stale, or every source
    // lacks the object — leave the plan untouched so the caller still reports a
    // real status instead of failing on an empty route list.
    let kept = routes
        .iter()
        .filter(|route| {
            !object_miss_key(&route.url).is_some_and(|key| live.contains(&key))
        })
        .cloned()
        .collect::<Vec<_>>();
    if kept.is_empty() {
        tracing::debug!(
            route_count = routes.len(),
            "Every route is memoised as missing this object; retrying the plan"
        );
        return;
    }
    *routes = kept;
}

#[cfg(test)]
mod object_miss_tests {
    use super::*;
    use crate::util::fetch::DownloadRouteSource;

    fn route(url: &str) -> DownloadRoute {
        DownloadRoute {
            url: url.to_string(),
            source: DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: ProxyPolicy::Direct,
        }
    }

    #[test]
    fn a_remembered_miss_is_dropped_but_the_plan_is_never_emptied() {
        let mut routes = vec![
            route("https://miss-a.example/repository/public/a/b.jar"),
            route("https://miss-b.example/repository/public/a/b.jar"),
        ];
        remember_object_miss(&routes[0].url);
        drop_known_object_misses(&mut routes);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].url, "https://miss-b.example/repository/public/a/b.jar",
            "only the route known to miss this object is dropped"
        );

        // When every route is memoised, the plan survives: a stale memo must
        // not turn a recoverable 404 into an empty route list.
        remember_object_miss(&routes[0].url);
        drop_known_object_misses(&mut routes);
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn an_unexpired_miss_is_known_and_a_different_path_is_not() {
        remember_object_miss("https://memo.example/repository/public/x/1.jar");
        assert!(object_miss_is_known(
            "https://memo.example/repository/public/x/1.jar"
        ));
        assert!(!object_miss_is_known(
            "https://memo.example/repository/public/x/2.jar"
        ));
        assert!(
            !object_miss_is_known(
                "https://other.example/repository/public/x/1.jar"
            ),
            "a miss belongs to one authority, not to the path alone"
        );
    }
}
