//! Shared authority stream admission.
//!
//! Three budgets bound an HTTP/2 stream, and they answer different questions:
//!
//! * the caller's `FetchSemaphore`, when supplied, is the resource weight the
//!   stream's bytes are charged against — the same weight a legacy transfer of
//!   the same file would take;
//!
//! * the per-authority semaphore here is a **fixed outer bound** on how many
//!   streams one authority may multiplex at once; and
//! * [`super::host_admission`] is the **learned** per-host allowance. It is fed
//!   by the same controller the plain transfer path uses, so a host whose knee
//!   is low is not handed thirty-two multiplexed streams just because they
//!   share one socket. The learned limit stays in charge; the stream allowance
//!   scales it by the measured multiplexing ratio and caps it at the measured
//!   per-host batch knee, so a host that has earned eight sockets gets sixteen
//!   streams and one that has earned the small-file ceiling gets the full
//!   [`super::constants::H2_STREAMS_PER_HOST_CAP`].
//!
//! Streams are also allocated across connections at most
//! [`super::constants::H2_STREAMS_PER_CONNECTION_CAP`] at a time (see
//! `AuthorityScheduler::least_loaded`); a full connection is what asks the pool
//! for a sibling TCP connection.

use crate::util::fetch::{DownloadRoute, ProxyPolicy};
use parking_lot::Mutex;
use std::sync::{Arc, LazyLock};
use tokio::sync::{
    AcquireError, OwnedSemaphorePermit, Semaphore, SemaphorePermit,
};

use super::bounded_table::BoundedTable;
use super::constants::{
    ASSET_RESOURCE_WAIT_TIMEOUT, MAX_ASSET_H2_STREAMS_PER_AUTHORITY,
    MAX_H2_STREAMS_PER_AUTHORITY, RESOURCE_WAIT_TIMEOUT,
};
use super::host_admission::HostStreamPermit;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AuthorityKey {
    authority: String,
    proxy: ProxyPolicy,
    asset: bool,
}

const STREAM_BUDGET_TABLE_CAPACITY: usize = 256;

static AUTHORITY_BUDGETS: LazyLock<
    Mutex<BoundedTable<AuthorityKey, Arc<Semaphore>>>,
> = LazyLock::new(|| {
    Mutex::new(BoundedTable::new(STREAM_BUDGET_TABLE_CAPACITY))
});

/// A stream budget may be forgotten once no permit holds it: an evicted budget
/// whose permits are still open would admit a second full allowance for the
/// same authority.
fn budget_is_droppable(budget: &Arc<Semaphore>) -> bool {
    Arc::strong_count(budget) == 1
}

pub(crate) struct H2StreamPermit {
    _authority: Option<OwnedSemaphorePermit>,
    /// The serving host's learned multiplexed allowance.
    ///
    /// Held for the life of the stream so the host controller sees the
    /// multiplexed path, exactly as a plain transfer's `HostPermit` does.
    _host: HostStreamPermit,
}

pub(crate) struct H2DownloadPermit<'a> {
    _stream: H2StreamPermit,
    _file: Option<SemaphorePermit<'a>>,
}

/// Why a multiplexed stream could not be admitted.
#[derive(Debug)]
pub(crate) enum H2StreamAdmissionError {
    /// The per-authority stream budget is closed.
    Budget(AcquireError),
    /// The serving host's learned allowance has no free stream slot.
    Host(super::host_admission::AdmissionError),
}

impl From<H2StreamAdmissionError> for crate::ErrorKind {
    fn from(error: H2StreamAdmissionError) -> Self {
        match error {
            H2StreamAdmissionError::Budget(error) => {
                crate::ErrorKind::AcquireError(error)
            }
            H2StreamAdmissionError::Host(error) => error.into(),
        }
    }
}

pub(crate) async fn acquire_download<'a>(
    route: &DownloadRoute,
    semaphore: Option<&'a crate::util::fetch::FetchSemaphore>,
    asset: bool,
) -> Result<H2DownloadPermit<'a>, H2StreamAdmissionError> {
    let stream = acquire_for(route, asset).await?;
    let file = match semaphore {
        Some(semaphore) => Some(
            semaphore
                .0
                .acquire()
                .await
                .map_err(H2StreamAdmissionError::Budget)?,
        ),
        None => None,
    };
    Ok(H2DownloadPermit {
        _stream: stream,
        _file: file,
    })
}

fn budget_for(route: &DownloadRoute, asset: bool) -> Option<Arc<Semaphore>> {
    let authority = crate::util::fetch::url_authority(&route.url)?;
    let key = AuthorityKey {
        authority: super::proxy_context::authority_key(&authority, route.proxy),
        proxy: route.proxy,
        asset,
    };
    let mut budgets = AUTHORITY_BUDGETS.lock();
    Some(budgets.get_or_insert_with(
        key,
        || {
            Arc::new(Semaphore::new(if asset {
                MAX_ASSET_H2_STREAMS_PER_AUTHORITY
            } else {
                MAX_H2_STREAMS_PER_AUTHORITY
            }))
        },
        budget_is_droppable,
    ))
}

/// Admits one multiplexed stream against both budgets.
///
/// The learned host allowance is taken first and the fixed authority semaphore
/// second. That order matters: the host allowance is the tighter of the two by
/// construction — at most [`super::constants::H2_STREAMS_PER_HOST_CAP`] streams,
/// against an authority bound of
/// [`MAX_ASSET_H2_STREAMS_PER_AUTHORITY`] asset or
/// [`MAX_H2_STREAMS_PER_AUTHORITY`] range streams — so taking it first means a
/// waiter never holds a slot in the looser budget while it waits on the tighter
/// one. The authority semaphore then only narrows what the host already
/// admitted, which is exactly the "outer bound" relationship the two are meant
/// to have.
pub(crate) async fn acquire_for(
    route: &DownloadRoute,
    asset: bool,
) -> Result<H2StreamPermit, H2StreamAdmissionError> {
    let host = super::host_admission::acquire_stream(
        route,
        if asset {
            ASSET_RESOURCE_WAIT_TIMEOUT
        } else {
            RESOURCE_WAIT_TIMEOUT
        },
    )
    .await
    .map_err(H2StreamAdmissionError::Host)?;
    let authority = match budget_for(route, asset) {
        Some(budget) => Some(
            budget
                .acquire_owned()
                .await
                .map_err(H2StreamAdmissionError::Budget)?,
        ),
        None => None,
    };
    Ok(H2StreamPermit {
        _authority: authority,
        _host: host,
    })
}

/// Free learned stream slots on the host a route would land on.
///
/// Exposed so the asset batch can decide, before queueing hundreds of streams,
/// whether the host's learned allowance is already saturated.
#[cfg(test)]
pub(crate) fn available_host_streams(route: &DownloadRoute) -> usize {
    super::host_admission::stream_headroom(route)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::download::constants::{
        H2_STREAMS_PER_HOST_CAP, H2_STREAMS_PER_LEARNED_SOCKET,
        HOST_CONCURRENCY_INITIAL,
    };
    use crate::util::fetch::{DownloadRouteSource, FetchSemaphore};

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

    #[tokio::test]
    async fn saturated_host_does_not_reserve_global_weights() {
        // The learned host allowance is the binding constraint for streams, so
        // it is the interesting saturation here: a pending stream must not hold
        // a global transfer weight while it waits, or a busy authority would
        // starve the large transfers those weights exist to protect.
        let busy = route("busy-weight.invalid");
        let other = route("other-weight.invalid");
        let global = FetchSemaphore(Semaphore::new(2));
        let mut held = Vec::new();
        for _ in 0..available_host_streams(&busy) {
            held.push(acquire_for(&busy, false).await.unwrap());
        }
        let pending = acquire_download(&busy, Some(&global), false);
        tokio::pin!(pending);
        assert!(futures::poll!(pending.as_mut()).is_pending());
        assert_eq!(global.0.available_permits(), 2);
        let available =
            acquire_download(&other, Some(&global), true).await.unwrap();
        assert_eq!(global.0.available_permits(), 1);
        drop(available);
        drop(pending);
        drop(held);
        assert_eq!(global.0.available_permits(), 2);
    }

    #[tokio::test]
    async fn ranges_and_assets_share_the_same_global_weights() {
        let global = FetchSemaphore(Semaphore::new(3));
        let target = route("shared-weights.invalid");
        let first = acquire_download(&target, Some(&global), false)
            .await
            .unwrap();
        let second = acquire_download(&target, Some(&global), false)
            .await
            .unwrap();
        let third = acquire_download(&target, Some(&global), true)
            .await
            .unwrap();
        assert_eq!(global.0.available_permits(), 0);
        drop((first, second, third));
        assert_eq!(global.0.available_permits(), 3);
    }

    #[tokio::test]
    async fn a_host_learned_limit_gates_multiplexed_streams() {
        // A fresh host learns four sockets, and the multiplexed allowance is
        // that number scaled by the measured socket-to-stream ratio — never
        // more than the measured per-host batch knee, and never just the raw
        // limit, which would hand multiplexing less than the plain path already
        // delivers.
        let target = route("stream-gate.invalid");
        let expected = (HOST_CONCURRENCY_INITIAL
            * H2_STREAMS_PER_LEARNED_SOCKET)
            .min(H2_STREAMS_PER_HOST_CAP);
        assert_eq!(available_host_streams(&target), expected);
        let mut held = Vec::new();
        for _ in 0..expected {
            held.push(acquire_for(&target, true).await.unwrap());
        }
        assert_eq!(available_host_streams(&target), 0);
        let pending = acquire_for(&target, true);
        tokio::pin!(pending);
        assert!(futures::poll!(pending.as_mut()).is_pending());
        drop(held);
        assert!(pending.await.is_ok());
    }

    #[tokio::test]
    async fn stream_completions_grow_a_host_the_transfer_guard_would_call_idle()
    {
        // The regression this guards: the app-limited guard used to read
        // `in_flight` unconditionally, but a multiplexed asset batch only ever
        // increments `stream_in_flight`. Every stream completion therefore saw
        // an idle host, returned before growing, and the host stayed at its
        // cold-start limit for the whole session — pinning the stream allowance
        // at eight against a measured knee of 64.
        let target = route("stream-growth.invalid");
        let before = available_host_streams(&target);
        // Fill the stream allowance so the guard's "fewer than half in flight"
        // condition is satisfied, exactly as a saturated asset batch does.
        let mut held = Vec::new();
        for _ in 0..before {
            held.push(acquire_for(&target, true).await.unwrap());
        }
        for _ in 0..64 {
            crate::util::download::host_admission::record_stream_completion(
                &target.url,
                target.proxy,
                Some(256 * 1024),
                std::time::Duration::from_millis(40),
            );
        }
        drop(held);
        assert!(
            available_host_streams(&target) > before,
            "a saturated stream path must be able to grow the host"
        );
    }
}
