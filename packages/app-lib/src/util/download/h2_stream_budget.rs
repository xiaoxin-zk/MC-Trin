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
//!   scales it by the measured multiplexing ratio, so a host whose limit grows
//!   to eight reaches the measured sixteen-stream saturation point instead of
//!   being stuck at the four-stream knee of a plain socket plan.
//!
//! Streams are also allocated across connections at most
//! [`super::constants::H2_STREAMS_PER_CONNECTION_CAP`] at a time (see
//! `AuthorityScheduler::least_loaded`); a full connection is what asks the pool
//! for a sibling TCP connection.

use crate::util::fetch::{DownloadRoute, ProxyPolicy};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::{
    AcquireError, OwnedSemaphorePermit, Semaphore, SemaphorePermit,
};

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

static AUTHORITY_BUDGETS: LazyLock<
    Mutex<HashMap<AuthorityKey, Arc<Semaphore>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
    if budgets.len() >= 256 {
        budgets.retain(|_, budget| Arc::strong_count(budget) > 1);
    }
    Some(
        budgets
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Semaphore::new(if asset {
                    MAX_ASSET_H2_STREAMS_PER_AUTHORITY
                } else {
                    MAX_H2_STREAMS_PER_AUTHORITY
                }))
            })
            .clone(),
    )
}

/// Admits one multiplexed stream against both budgets.
///
/// The learned host allowance is taken first and the fixed authority semaphore
/// second. That order matters: the host allowance is always the tighter of the
/// two (at most one connection's saturation point, against an authority bound
/// of 32 asset streams or 256 range streams), so taking it first means a waiter
/// never holds a slot in the looser budget while it waits on the tighter one.
/// The authority semaphore then only narrows what the host already admitted,
/// which is exactly the "outer bound" relationship the two are meant to have.
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
        H2_STREAMS_PER_CONNECTION_CAP, HOST_CONCURRENCY_INITIAL,
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
        let available = acquire_download(&other, Some(&global), true)
            .await
            .unwrap();
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
        // more than one connection's saturation point, and never just the raw
        // limit, which would hand multiplexing less than the plain path already
        // delivers.
        let target = route("stream-gate.invalid");
        let expected = (HOST_CONCURRENCY_INITIAL
            * crate::util::download::constants::H2_STREAMS_PER_LEARNED_SOCKET)
            .min(H2_STREAMS_PER_CONNECTION_CAP);
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
}
