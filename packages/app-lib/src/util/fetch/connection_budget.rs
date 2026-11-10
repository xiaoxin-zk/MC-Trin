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

pub(super) async fn acquire_initial_segment_permits<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
    count: usize,
) -> crate::Result<Vec<NativeConnectionPermit<'a>>> {
    let queue_started = Instant::now();
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
        });
    }
    tracing::debug!(
        queue_wait_ms = queue_started.elapsed().as_millis(),
        actual_segments = permits.len(),
        "Acquired initial segmented download permits"
    );
    Ok(permits)
}

pub(crate) struct NativeConnectionPermit<'a> {
    _global: SemaphorePermit<'a>,
    _native: crate::util::download::native_budget::NativeBudgetPermit,
}

pub(crate) async fn acquire_native_connection<'a>(
    route: &DownloadRoute,
    semaphore: &'a FetchSemaphore,
) -> crate::Result<NativeConnectionPermit<'a>> {
    let native = crate::util::download::native_budget::acquire(route).await?;
    let global = semaphore.0.acquire().await?;
    Ok(NativeConnectionPermit {
        _global: global,
        _native: native,
    })
}

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
    })
}
