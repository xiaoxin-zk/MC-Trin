use super::*;

pub(super) fn segmented_concurrency_cap(available_permits: usize) -> usize {
    available_permits.min(MAX_SEGMENT_CONCURRENCY)
}

pub(super) fn route_segmented_concurrency_cap(
    route: &DownloadRoute,
    available_permits: usize,
    size: u64,
) -> usize {
    let fair_share = if size >= BULK_SEGMENTED_DOWNLOAD_THRESHOLD {
        let reserved = BULK_DOWNLOAD_RESERVED_PERMITS
            .min(available_permits / DOWNLOAD_FAIR_SHARE_DIVISOR);
        available_permits.saturating_sub(reserved).max(1)
    } else {
        (available_permits / DOWNLOAD_FAIR_SHARE_DIVISOR).max(1)
    };

    segmented_concurrency_cap(available_permits)
        .min(fair_share)
        // The host's measured knee is the real ceiling: `maven.aliyun.com`
        // stops at 16 and `cmcc.mirrors.ustc.edu.cn` at 8 whatever the caller
        // asks for, while an unmeasured host keeps the generic bulk ceiling.
        // Planning is deliberately optimistic; `host_admission` enforces the
        // learned limit per transfer as soon as it is measured.
        .min(crate::util::download::host_admission::capacity(route))
        .min(crate::util::download::native_budget::capacity(route))
}

/// The user's configured download concurrency for a transfer budget.
///
/// Every production caller passes one of the state's semaphores, whose permit
/// count is the configured concurrency; that value is stable for the whole
/// session and is what the range cap must divide up. A semaphore created ad hoc
/// has no recorded configuration, so its current free permits are the only
/// upper bound available — and that number shrinks as transfers start, which
/// would silently reduce a later file's segment count. Callers that need a
/// stable cap must therefore pass a state semaphore, not a private one.
pub(crate) fn configured_semaphore_limit(semaphore: &FetchSemaphore) -> usize {
    if let Some(state) = crate::State::get_if_initialized()
        && (std::ptr::eq(
            &raw const state.fetch_semaphore.0,
            &raw const semaphore.0,
        ) || std::ptr::eq(
            &raw const state.download_semaphore.0,
            &raw const semaphore.0,
        ) || std::ptr::eq(
            &raw const state.api_semaphore.0,
            &raw const semaphore.0,
        ))
    {
        return state.download_concurrency();
    }
    let available = semaphore.0.available_permits().max(1);
    tracing::debug!(
        available_permits = available,
        "Transfer budget is not a state semaphore; its concurrency is only known from free permits"
    );
    available
}

/// Admits one file's range plan: the host's transfer slot plus one socket per
/// range.
///
/// The host slot is charged **once per file**, not once per range. A range is a
/// socket, and sockets are what [`native_budget`] bounds; charging each range
/// against the learned transfer limit would re-impose the cold-start limit of
/// four on every range plan and turn a 200 MB archive into a four-way download,
/// which is exactly what `host_admission::capacity` exists to avoid. The slot
/// must outlive the plan, so it is returned separately for the caller to hold.
pub(super) async fn acquire_initial_segment_permits<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
    count: usize,
    size: Option<u64>,
) -> crate::Result<(
    crate::util::download::host_admission::HostPermit,
    Vec<NativeConnectionPermit<'a>>,
)> {
    let queue_started = Instant::now();
    let host = crate::util::download::host_admission::acquire(
        route,
        size,
        RESOURCE_WAIT_TIMEOUT,
    )
    .await
    .map_err(|error| {
        crate::Error::from(crate::ErrorKind::NetworkError(format!(
            "{}: {error}",
            route.url
        )))
    })?;
    // Start with the available share instead of waiting for an entire batch.
    // The scheduler retains its configured ceiling and grows as slots return.
    let count = count
        .min(crate::util::download::native_budget::available(route).max(1))
        .min(semaphore.0.available_permits().max(1));
    let native_permits =
        crate::util::download::native_budget::acquire_many(route, count)
            .await?;
    let mut global = semaphore.0.acquire_many(count as u32).await?;
    let mut permits = Vec::with_capacity(count);
    for native in native_permits {
        let global = global
            .split(1)
            .expect(FETCH_FETCH_PERMIT_BATCH_HAS_ENOUGH_PERMITS);
        permits.push(NativeConnectionPermit {
            _global: global,
            _native: native,
            _host: None,
        });
    }
    if permits.is_empty() {
        return Err(crate::ErrorKind::NetworkError(
            "the serving host has no spare connection slots".to_string(),
        )
        .into());
    }
    tracing::debug!(
        queue_wait_ms = queue_started.elapsed().as_millis(),
        actual_segments = permits.len(),
        "Acquired initial segmented download permits"
    );
    Ok((host, permits))
}

pub(crate) struct NativeConnectionPermit<'a> {
    _global: SemaphorePermit<'a>,
    _native: crate::util::download::native_budget::NativeBudgetPermit,
    /// The serving host's learned transfer slot.
    ///
    /// `Some` only on the permit that admitted the file; extra sockets an
    /// already-admitted transfer grows into leave it `None` because the file's
    /// one transfer slot is held elsewhere for the life of the plan.
    _host: Option<crate::util::download::host_admission::HostPermit>,
}

/// One file transfer's admission: a physical socket plus a slot in the serving
/// host's learned transfer limit.
///
/// The two are deliberately separate. The socket budget is per authority and
/// fixed, because a socket is a socket whether it carries one transfer or
/// sixteen multiplexed streams. The transfer limit is learned per post-redirect
/// host, because the measured capacities differ by two orders of magnitude.
pub(crate) async fn acquire_native_connection<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
    size: Option<u64>,
) -> crate::Result<NativeConnectionPermit<'a>> {
    let host = crate::util::download::host_admission::acquire(
        route,
        size,
        RESOURCE_WAIT_TIMEOUT,
    )
    .await
    .map_err(|error| {
        crate::Error::from(crate::ErrorKind::NetworkError(format!(
            "{}: {error}",
            route.url
        )))
    })?;
    let native = crate::util::download::native_budget::acquire(route).await?;
    let global = semaphore.0.acquire().await?;
    Ok(NativeConnectionPermit {
        _global: global,
        _native: native,
        _host: Some(host),
    })
}

/// One more socket for an existing transfer.
///
/// Range expansion, tail hedges and the batch-tail recovery plan all run inside
/// a file that already holds its host transfer slot, so they pay for the socket
/// and the configured transfer budget only. Taking a second host slot here would
/// make the host's learned limit cap how many ranges one file may grow to.
pub(super) fn try_acquire_native_connection<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
) -> Option<NativeConnectionPermit<'a>> {
    let native =
        crate::util::download::native_budget::try_acquire(route).ok()?;
    let global = semaphore.0.try_acquire().ok()?;
    Some(NativeConnectionPermit {
        _global: global,
        _native: native,
        _host: None,
    })
}

/// Blocks for one more socket inside an already-admitted transfer.
pub(crate) async fn acquire_socket<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
) -> crate::Result<NativeConnectionPermit<'a>> {
    let native = crate::util::download::native_budget::acquire(route).await?;
    let global = semaphore.0.acquire().await?;
    Ok(NativeConnectionPermit {
        _global: global,
        _native: native,
        _host: None,
    })
}
