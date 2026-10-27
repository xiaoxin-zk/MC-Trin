//! Range downloads with independent retries, HTTP/1 fallback, and durable checkpoints.

use super::h2_download::{H2DownloadFailure, H2DownloadOutcome};
use super::h2_pool::SharedH2Connection;
use super::range_journal::{Checkpoint, RangeJournal};
use crate::util::download::constants::{
    DIAGNOSTICS_RANGE, DOWNLOAD_SCHEDULER_INTERVAL, H2_DOWNLOAD_INVALID_JAR,
    H2_RANGE_INCORRECT_SIZE, H2_RANGE_INVALID_RANGE_RESUME_URL,
    H2_RANGE_MAX_WORKERS, H2_RANGE_RETRY_ATTEMPTS, H2_RANGE_RETRY_DELAY_MS,
    IDENTITY_ENCODING, RANGE_CHECKPOINT_BYTES, RESOURCE_WAIT_TIMEOUT,
};
use crate::util::fetch::{
    self, DownloadRequest, DownloadResult, DownloadRoute,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use http::header::{ACCEPT_ENCODING, RANGE};
use http::{HeaderValue, StatusCode, Uri};
use parking_lot::Mutex;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

struct H2Range {
    start: u64,
    end: u64,
}

struct PartCleanupGuard {
    path: PathBuf,
    armed: bool,
}
impl PartCleanupGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for PartCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
            let _ =
                std::fs::remove_file(super::range_journal::path(&self.path));
        }
    }
}

#[async_trait]
trait RangeTransport: Sync {
    async fn open(
        &self,
        start: u64,
        end: u64,
        attempt: usize,
    ) -> Result<RangeResponse<'_>, H2DownloadFailure>;
    fn failed(&self, _identity: &RangeIdentity, _failure: H2DownloadFailure) {}
    fn on_slow(
        &self,
        _identity: &RangeIdentity,
        _speed: u64,
        _remaining: u64,
        _region: std::ops::Range<u64>,
        _pressure: super::local_resources::Pressure,
    ) -> bool {
        false
    }
}

struct RangeIdentity {
    h2: bool,
    peer: Option<std::net::SocketAddr>,
    connection: Option<Arc<SharedH2Connection>>,
    aggregate_window: Option<Mutex<super::h2_receive::ConnectionRateWindow>>,
}

struct RangeResponse<'a> {
    body: BoxStream<'a, Result<Bytes, H2DownloadFailure>>,
    identity: RangeIdentity,
}

impl<'a> RangeResponse<'a> {
    #[cfg(test)]
    fn fixture(
        body: BoxStream<'a, Result<Bytes, H2DownloadFailure>>,
        h2: bool,
    ) -> Self {
        Self {
            body,
            identity: RangeIdentity {
                h2,
                peer: None,
                connection: None,
                aggregate_window: None,
            },
        }
    }
}

struct NativeRangeTransport<'a> {
    uri: Uri,
    request: &'a DownloadRequest,
    route: &'a DownloadRoute,
    total_size: u64,
    semaphore: Option<&'a fetch::FetchSemaphore>,
    fallback_budget: fetch::FetchSemaphore,
    used_http1: std::sync::atomic::AtomicBool,
    protocol_failed: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl RangeTransport for NativeRangeTransport<'_> {
    async fn open(
        &self,
        start: u64,
        end: u64,
        attempt: usize,
    ) -> Result<RangeResponse<'_>, H2DownloadFailure> {
        let _ = attempt;
        if !self.protocol_failed.load(Ordering::Relaxed)
            && fetch::url_authority(&self.route.url).is_some_and(|authority| {
                super::native_reputation::transport_enabled(
                    &authority,
                    self.route.proxy,
                    super::native_reputation::NativeTransport::H2MultiRange,
                )
            })
            && let Ok(connection) =
                super::h2_pool::shared_batch_connection(self.route, true).await
        {
            let permit = tokio::time::timeout(
                RESOURCE_WAIT_TIMEOUT,
                super::h2_stream_budget::acquire_download(
                    self.route,
                    self.semaphore,
                    false,
                ),
            )
            .await
            .map_err(|_| H2DownloadFailure::Connect)?
            .map_err(|_| H2DownloadFailure::Connect)?;
            let permit = (permit, connection.track_stream());
            let mut headers =
                super::h2_download::request_headers(self.request, self.route);
            headers.insert(
                ACCEPT_ENCODING,
                HeaderValue::from_static(IDENTITY_ENCODING),
            );
            headers.insert(
                RANGE,
                HeaderValue::from_str(&format!("bytes={start}-{end}"))
                    .map_err(|_| H2DownloadFailure::Http)?,
            );
            let opened = super::h2_download::open_stream(
                &connection,
                &self.uri,
                headers,
            )
            .await;
            let (response, body) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    let failure =
                        super::h2_download::classify_download_error(&error);
                    if failure == H2DownloadFailure::Protocol {
                        self.protocol_failed.store(true, Ordering::Relaxed);
                    }
                    return Err(failure);
                }
            };
            super::native_breaker::record_response_status_failure(
                self.route,
                response.status().as_u16(),
                fetch::retry_after_headers(response.headers()),
            );
            if response.status() != StatusCode::PARTIAL_CONTENT
                || !content_range_matches(
                    response.headers(),
                    &H2Range { start, end },
                    self.total_size,
                )
            {
                return Err(range_response_failure(response.status().as_u16()));
            }
            let connection_for_metrics = connection.clone();
            let stream = futures::stream::unfold(
                (Some(body), permit, connection_for_metrics),
                |(body, permit, connection_for_metrics)| async move {
                    let mut body = body?;
                    match super::h2_receive::receive_chunk(
                        &mut body,
                        DIAGNOSTICS_RANGE,
                    )
                    .await
                    {
                        Ok(Some(chunk)) => {
                            connection_for_metrics.record_bytes(chunk.len());
                            if super::h2_receive::release_capacity(
                                &mut body,
                                chunk.len(),
                            )
                            .is_err()
                            {
                                return Some((
                                    Err(H2DownloadFailure::Protocol),
                                    (None, permit, connection_for_metrics),
                                ));
                            }
                            Some((
                                Ok(chunk),
                                (Some(body), permit, connection_for_metrics),
                            ))
                        }
                        Ok(None) => None,
                        Err(error) => Some((
                            Err(super::h2_download::classify_download_error(
                                &error,
                            )),
                            (None, permit, connection_for_metrics),
                        )),
                    }
                },
            );
            return Ok(RangeResponse {
                body: Box::pin(stream),
                identity: RangeIdentity {
                    h2: true,
                    peer: connection.remote_addr,
                    aggregate_window: Some(Mutex::new(
                        super::h2_receive::ConnectionRateWindow::new(
                            connection.bytes_transferred_for_scheduler(),
                            std::time::Instant::now(),
                        ),
                    )),
                    connection: Some(connection),
                },
            });
        }
        self.used_http1.store(true, Ordering::Relaxed);
        let semaphore = self.semaphore.unwrap_or(&self.fallback_budget);
        let permit = tokio::time::timeout(
            RESOURCE_WAIT_TIMEOUT,
            fetch::acquire_native_connection(self.route, semaphore),
        )
        .await
        .map_err(|_| H2DownloadFailure::Connect)?
        .map_err(|_| H2DownloadFailure::Connect)?;
        let mut clients = fetch::DownloadClients::for_request(
            &fetch::HTTP1_NO_REDIRECT_REQWEST_CLIENT,
            &fetch::HTTP1_DIRECT_REQWEST_CLIENT,
        );
        clients.system = clients.http1_system.clone();
        clients.direct = clients.http1_direct.clone();
        clients.force_http1 = true;
        let (response, _) =
            super::native_request::send_path_request_with_clients(
                self.route,
                self.request.header.as_ref(),
                None,
                self.request.download_meta.as_ref(),
                Some(start),
                Some(end),
                &clients,
                None,
            )
            .await
            .map_err(|error| {
                super::h2_download::classify_download_error(&error)
            })?;
        let expected = format!("bytes {start}-{end}/{}", self.total_size);
        super::native_breaker::record_response_status_failure(
            self.route,
            response.status().as_u16(),
            fetch::retry_after(&response),
        );
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
            || response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                != Some(expected.as_str())
        {
            return Err(range_response_failure(response.status().as_u16()));
        }
        let peer = response.remote_addr();
        let feedback = response
            .extensions()
            .get::<super::endpoint_selection::metrics::EndpointTransferGuard>()
            .cloned();
        let stream = futures::stream::unfold(
            (Some(response), permit, feedback),
            |(response, permit, feedback)| async move {
                let mut response = response?;
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if let Some(feedback) = &feedback {
                            feedback.record(
                                chunk.len(),
                                super::local_resources::network_pressure(),
                            );
                        }
                        Some((Ok(chunk), (Some(response), permit, feedback)))
                    }
                    Ok(None) => None,
                    Err(error) => Some((
                        Err(super::h2_download::classify_download_error(
                            &error.into(),
                        )),
                        (None, permit, feedback),
                    )),
                }
            },
        );
        Ok(RangeResponse {
            body: Box::pin(stream),
            identity: RangeIdentity {
                h2: false,
                peer,
                connection: None,
                aggregate_window: None,
            },
        })
    }

    fn failed(&self, identity: &RangeIdentity, failure: H2DownloadFailure) {
        if identity.h2
            && failure == H2DownloadFailure::Protocol
            && let Some(authority) = fetch::url_authority(&self.route.url)
        {
            self.protocol_failed.store(true, Ordering::Relaxed);
            super::native_reputation::record_transport_failure(
                &authority,
                self.route.proxy,
                super::native_reputation::NativeTransport::H2MultiRange,
            );
        }
    }

    fn on_slow(
        &self,
        identity: &RangeIdentity,
        speed: u64,
        remaining: u64,
        region: std::ops::Range<u64>,
        pressure: super::local_resources::Pressure,
    ) -> bool {
        let Some(clients) = super::proxy_context::clients() else {
            return false;
        };
        let protocol = if identity.h2 {
            super::endpoint_selection::Protocol::H2
        } else {
            super::endpoint_selection::Protocol::Http1
        };
        let aggregate = identity
            .connection
            .as_ref()
            .zip(identity.aggregate_window.as_ref())
            .and_then(|(connection, window)| {
                window.lock().observe(
                    connection.bytes_transferred_for_scheduler(),
                    std::time::Instant::now(),
                )
            });
        let Some(speed) = super::h2_receive::endpoint_speed(
            speed,
            identity
                .connection
                .as_ref()
                .map_or(1, |connection| connection.active_streams()),
            aggregate,
        ) else {
            return false;
        };
        clients.endpoints.on_slow_region(
            self.request,
            self.route,
            &clients,
            protocol,
            identity.peer.map(|a| a.ip()),
            region,
            Some(speed),
            pressure,
        );
        clients
            .endpoints
            .recovery_lease(
                self.request,
                self.route,
                protocol,
                identity.peer,
                speed,
                remaining,
            )
            .is_some()
    }
}

pub(crate) async fn download(
    _connection: &Arc<SharedH2Connection>,
    uri: &Uri,
    request: &DownloadRequest,
    route: &DownloadRoute,
    destination: &Path,
    part_path: &Path,
    total_size: u64,
    concurrency: usize,
    semaphore: Option<&fetch::FetchSemaphore>,
) -> H2DownloadOutcome {
    let transport = NativeRangeTransport {
        uri: uri.clone(),
        request,
        route,
        total_size,
        semaphore,
        fallback_budget: fetch::FetchSemaphore(tokio::sync::Semaphore::new(8)),
        used_http1: AtomicBool::default(),
        protocol_failed: AtomicBool::default(),
    };
    let started = std::time::Instant::now();
    let outcome = run_download(
        &transport,
        request,
        route,
        destination,
        part_path,
        total_size,
        concurrency,
        None,
    )
    .await;
    if let H2DownloadOutcome::Completed(result) = &outcome
        && !transport.used_http1.load(Ordering::Relaxed)
        && let Some(authority) = fetch::url_authority(&route.url)
    {
        super::native_reputation::record_transport_success(
            &authority,
            route.proxy,
            super::native_reputation::NativeTransport::H2MultiRange,
            result.size as f64 / started.elapsed().as_secs_f64().max(0.001),
        );
    }
    outcome
}

pub(crate) async fn resume_http1(
    request: &DownloadRequest,
    route: &DownloadRoute,
    destination: &Path,
    part: &Path,
    semaphore: &fetch::FetchSemaphore,
) -> crate::Result<Option<H2DownloadOutcome>> {
    if request
        .cancellation
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Ok(Some(H2DownloadOutcome::Canceled));
    }
    if !tokio::fs::try_exists(super::range_journal::path(part)).await? {
        return Ok(None);
    }
    let Some(size) = request.integrity.size else {
        discard(part).await?;
        return Ok(None);
    };
    let load = RangeJournal::load(part, &request.integrity, size);
    let loaded = if let Some(cancellation) = request.cancellation.as_ref() {
        tokio::select! { biased; _ = cancellation.cancelled() => return Ok(Some(H2DownloadOutcome::Canceled)), result = load => result? }
    } else {
        load.await?
    };
    let Some(journal) = loaded else {
        discard(part).await?;
        return Ok(None);
    };
    let count = journal
        .ranges()
        .await
        .len()
        .min((fetch::configured_semaphore_limit(semaphore) / 2).max(1))
        .min(super::local_resources::range_limit(part).await.max(1));
    let transport = NativeRangeTransport {
        uri: route.url.parse().map_err(|_| {
            crate::ErrorKind::InputError(
                H2_RANGE_INVALID_RANGE_RESUME_URL.into(),
            )
        })?,
        request,
        route,
        total_size: size,
        semaphore: Some(semaphore),
        fallback_budget: fetch::FetchSemaphore(tokio::sync::Semaphore::new(8)),
        used_http1: AtomicBool::default(),
        protocol_failed: AtomicBool::default(),
    };
    Ok(Some(
        run_download(
            &transport,
            request,
            route,
            destination,
            part,
            size,
            count,
            Some(journal),
        )
        .await,
    ))
}

pub(crate) async fn discard(part: &Path) -> crate::Result<()> {
    for path in [part.to_path_buf(), super::range_journal::path(part)] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn range_response_failure(status: u16) -> H2DownloadFailure {
    if matches!(status, 200 | 206 | 416) {
        H2DownloadFailure::RangeUnsupported
    } else {
        H2DownloadFailure::Http
    }
}

async fn run_download(
    transport: &dyn RangeTransport,
    request: &DownloadRequest,
    route: &DownloadRoute,
    destination: &Path,
    part: &Path,
    size: u64,
    concurrency: usize,
    loaded_journal: Option<Arc<RangeJournal>>,
) -> H2DownloadOutcome {
    if request
        .cancellation
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return H2DownloadOutcome::Canceled;
    }
    if size == 0 {
        return H2DownloadOutcome::Fallback {
            failure: H2DownloadFailure::Content,
            preserve_partial: false,
        };
    }
    let mut cleanup = PartCleanupGuard::new(part);
    let loaded = match loaded_journal {
        Some(journal) => Some(journal),
        None => {
            let load = RangeJournal::load(part, &request.integrity, size);
            let result = if let Some(cancellation) =
                request.cancellation.as_ref()
            {
                tokio::select! { biased; _ = cancellation.cancelled() => { cleanup.disarm(); return H2DownloadOutcome::Canceled; }, result = load => result }
            } else {
                load.await
            };
            match result {
                Ok(loaded) => loaded,
                Err(_) => {
                    cleanup.disarm();
                    return H2DownloadOutcome::Fallback {
                        failure: H2DownloadFailure::Io,
                        preserve_partial: true,
                    };
                }
            }
        }
    };
    let output = if loaded.is_some() {
        super::range_output::RangeOutput::reopen(part, size).await
    } else {
        super::range_output::RangeOutput::create(part, size).await
    };
    let Ok(output) = output else {
        return H2DownloadOutcome::Fallback {
            failure: H2DownloadFailure::Io,
            preserve_partial: false,
        };
    };
    let journal = loaded.unwrap_or_else(|| {
        RangeJournal::new(part, &request.integrity, size, concurrency)
    });
    if journal.persist().await.is_err() {
        return H2DownloadOutcome::Fallback {
            failure: H2DownloadFailure::Io,
            preserve_partial: false,
        };
    }
    let preserve = request.integrity.supports_resume();
    if preserve {
        cleanup.disarm();
    }
    let ranges = journal.ranges().await;
    let downloaded =
        AtomicU64::new(ranges.iter().map(|range| range.written).sum());
    let reported = Mutex::new(super::h2_receive::H2ProgressGate::new(size));
    let workers = concurrency.clamp(1, H2_RANGE_MAX_WORKERS);
    let mut tasks = futures::stream::iter(ranges.into_iter().enumerate())
        .map(|(index, range)| {
            recover_range(
                transport,
                request,
                part,
                &output,
                &journal,
                index,
                range,
                &downloaded,
                &reported,
                size,
            )
        })
        .buffer_unordered(workers);
    loop {
        let next = if let Some(cancellation) = request.cancellation.as_ref() {
            tokio::select! { biased; _ = cancellation.cancelled() => return H2DownloadOutcome::Canceled, next = tasks.next() => next }
        } else {
            tasks.next().await
        };
        match next {
            Some(Ok(())) => {}
            Some(Err(failure)) => {
                if failure == H2DownloadFailure::RangeUnsupported {
                    drop(tasks);
                    drop(output);
                    if discard(part).await.is_err() {
                        return H2DownloadOutcome::Fallback {
                            failure: H2DownloadFailure::Io,
                            preserve_partial: true,
                        };
                    }
                    return H2DownloadOutcome::Fallback {
                        failure,
                        preserve_partial: false,
                    };
                }
                return H2DownloadOutcome::Fallback {
                    failure,
                    preserve_partial: preserve,
                };
            }
            None => break,
        }
    }
    drop(tasks);
    drop(output);
    let verification = fetch::verify_file(part, &request.integrity);
    let result = if let Some(cancellation) = request.cancellation.as_ref() {
        tokio::select! { biased; _ = cancellation.cancelled() => return H2DownloadOutcome::Canceled, result = verification => result }
    } else {
        verification.await
    };
    if let Err(error) = result {
        let invalid = fetch::is_integrity_error(&error)
            || matches!(error.raw.as_ref(), crate::ErrorKind::JSONError(_))
            || matches!(error.raw.as_ref(), crate::ErrorKind::OtherError(message) if message.starts_with(H2_DOWNLOAD_INVALID_JAR) || message.starts_with(H2_RANGE_INCORRECT_SIZE));
        if invalid && discard(part).await.is_err() {
            return H2DownloadOutcome::Fallback {
                failure: H2DownloadFailure::Io,
                preserve_partial: true,
            };
        }
        return H2DownloadOutcome::Fallback {
            failure: if invalid {
                H2DownloadFailure::Integrity
            } else {
                H2DownloadFailure::Io
            },
            preserve_partial: preserve && !invalid,
        };
    }
    if fetch::finalize_download(part, destination).await.is_err() {
        return H2DownloadOutcome::Fallback {
            failure: H2DownloadFailure::Io,
            preserve_partial: preserve,
        };
    }
    cleanup.disarm();
    let _ = tokio::fs::remove_file(super::range_journal::path(part)).await;
    H2DownloadOutcome::Completed(DownloadResult {
        path: destination.to_path_buf(),
        url: route.url.clone(),
        source: route.source,
        size,
        attempts: 1,
        fallback_count: 0,
        verified_sha1: request.integrity.sha1.clone(),
        verified_sha512: request.integrity.sha512.clone(),
        verified_file: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn recover_range(
    transport: &dyn RangeTransport,
    request: &DownloadRequest,
    part: &Path,
    output: &super::range_output::RangeOutput,
    journal: &RangeJournal,
    index: usize,
    range: Checkpoint,
    downloaded: &AtomicU64,
    reported: &Mutex<super::h2_receive::H2ProgressGate>,
    total_size: u64,
) -> Result<(), H2DownloadFailure> {
    if range.start + range.written == range.end {
        return Ok(());
    }
    let mut offset = range.start + range.written;
    let mut hash = journal
        .hash_prefix(index, part, &range)
        .await
        .map_err(|_| H2DownloadFailure::Io)?;
    let mut writer = output
        .open_range(offset, range.end)
        .await
        .map_err(|_| H2DownloadFailure::Io)?;
    let activity = super::h2_receive::H2TransferActivity::begin();
    let mut last_checkpoint = offset;
    let mut last_failure = H2DownloadFailure::Protocol;
    for attempt in 0..H2_RANGE_RETRY_ATTEMPTS {
        let RangeResponse { mut body, identity } =
            match transport.open(offset, range.end - 1, attempt).await {
                Ok(body) => body,
                Err(error) => {
                    if matches!(
                        error,
                        H2DownloadFailure::RangeUnsupported
                            | H2DownloadFailure::Io
                    ) {
                        return Err(error);
                    }
                    last_failure = error;
                    continue;
                }
            };
        let mut failure = None;
        let mut slow =
            super::native_slow::NativeSlowPolicy::for_http2(offset, None);
        let mut timer = tokio::time::interval(DOWNLOAD_SCHEDULER_INTERVAL);
        let mut last_slow_speed = None;
        timer.tick().await;
        loop {
            let chunk = tokio::select! {
                chunk = body.next() => chunk,
                _ = async { if let Some(token) = &request.cancellation { token.cancelled().await } else { std::future::pending().await } } => {
                    failure = Some(H2DownloadFailure::Slow); break;
                }
                _ = timer.tick() => {
                    if let Some((connection, window)) = identity.connection.as_ref().zip(identity.aggregate_window.as_ref()) {
                        window.lock().observe(connection.bytes_transferred_for_scheduler(), std::time::Instant::now());
                    }
                    let pressure = super::local_resources::pressure(part);
                    match slow.observe_with_pressure(offset, range.end - offset, pressure) {
                        super::native_slow::SlowDecision::Probe { bytes_per_second } => {
                            last_slow_speed = Some(bytes_per_second);
                        }
                        super::native_slow::SlowDecision::Idle { .. } => { failure = Some(H2DownloadFailure::Slow); break; }
                        _ => {}
                    }
                    if let Some(speed) = last_slow_speed
                        && !pressure.blocks_expansion()
                        && transport.on_slow(&identity, speed, range.end - offset, offset..range.end, pressure) {
                        failure = Some(H2DownloadFailure::EndpointChanged); break;
                    }
                    continue;
                }
            };
            let Some(chunk) = chunk else { break };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };
            request.transfer_counters.received(chunk.len() as u64);
            if chunk.len() as u64 > range.end - offset {
                failure = Some(H2DownloadFailure::Protocol);
                break;
            }
            writer
                .write_next(&chunk)
                .await
                .map_err(|_| H2DownloadFailure::Io)?;
            hash.update(&chunk);
            offset += chunk.len() as u64;
            activity.record_bytes(chunk.len());
            let current = downloaded
                .fetch_add(chunk.len() as u64, Ordering::Relaxed)
                + chunk.len() as u64;
            let should_report =
                reported.lock().should_report(current, total_size);
            if should_report {
                super::h2_download::record_install_progress(
                    request,
                    current.min(total_size),
                    total_size,
                )
                .await;
            }
            if offset - last_checkpoint >= RANGE_CHECKPOINT_BYTES {
                writer.flush().await.map_err(|_| H2DownloadFailure::Io)?;
                journal
                    .checkpoint(index, offset - range.start, &hash)
                    .await
                    .map_err(|_| H2DownloadFailure::Io)?;
                request
                    .transfer_counters
                    .commit(range.start + range.written, offset);
                last_checkpoint = offset;
            }
        }
        drop(body);
        writer.flush().await.map_err(|_| H2DownloadFailure::Io)?;
        request
            .transfer_counters
            .commit(range.start + range.written, offset);
        journal
            .checkpoint(index, offset - range.start, &hash)
            .await
            .map_err(|_| H2DownloadFailure::Io)?;
        if offset == range.end {
            return Ok(());
        }
        last_failure = failure.unwrap_or(H2DownloadFailure::Protocol);
        transport.failed(&identity, last_failure);
        tracing::debug!(
            range_start = range.start,
            resume_offset = offset,
            attempt = attempt + 1,
            "Retrying only unfinished range bytes"
        );
        tokio::time::sleep(Duration::from_millis(
            H2_RANGE_RETRY_DELAY_MS * (attempt as u64 + 1),
        ))
        .await;
    }
    Err(last_failure)
}

#[cfg(test)]
fn split_ranges(size: u64, count: usize) -> Vec<H2Range> {
    let base = size / count as u64;
    let remainder = size % count as u64;
    let mut start = 0;
    (0..count)
        .map(|index| {
            let length = base + u64::from(index < remainder as usize);
            let range = H2Range {
                start,
                end: start + length - 1,
            };
            start += length;
            range
        })
        .collect()
}

fn content_range_matches(
    headers: &http::HeaderMap,
    range: &H2Range,
    total_size: u64,
) -> bool {
    headers
        .get(http::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                == format!("bytes {}-{}/{}", range.start, range.end, total_size)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::future::poll_fn;
    use http::{Response, StatusCode};
    use sha1_smol::Sha1;
    use std::sync::atomic::AtomicUsize;

    struct H2RangeFixture {
        connection: Arc<SharedH2Connection>,
        requests: Arc<Mutex<Vec<(u64, u64)>>>,
        server: tokio::task::JoinHandle<()>,
        driver: tokio::task::JoinHandle<()>,
    }

    struct H2RangeReply {
        status: StatusCode,
        content_range: Option<String>,
        body: Vec<u8>,
        reset_after_bytes: Option<usize>,
    }

    async fn start_h2_range_fixture<F>(
        route: &DownloadRoute,
        reply_for: F,
    ) -> H2RangeFixture
    where
        F: Fn(u64, u64, usize) -> H2RangeReply + Send + Sync + 'static,
    {
        let (client_io, server_io) = tokio::io::duplex(256 * 1024);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_ranges = requests.clone();
        let request_index = Arc::new(AtomicUsize::new(0));
        let server_request_index = request_index.clone();
        let reply_for = Arc::new(reply_for);
        let server_reply_for = reply_for.clone();
        let server = tokio::spawn(async move {
            let mut connection =
                h2::server::handshake(server_io).await.unwrap();
            while let Some(Ok((request, mut respond))) =
                connection.accept().await
            {
                let request_ranges = request_ranges.clone();
                let request_index = server_request_index.clone();
                let reply_for = server_reply_for.clone();
                tokio::spawn(async move {
                    let range = request
                        .headers()
                        .get(http::header::RANGE)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .strip_prefix("bytes=")
                        .unwrap()
                        .to_string();
                    let (start, end) = range.split_once('-').unwrap();
                    let start = start.parse::<u64>().unwrap();
                    let end = end.parse::<u64>().unwrap();
                    request_ranges.lock().push((start, end));
                    let index = request_index.fetch_add(1, Ordering::Relaxed);
                    let reply = reply_for(start, end, index);
                    let declared_length = reply.body.len() as u64;
                    let mut response =
                        Response::builder().status(reply.status).header(
                            http::header::CONTENT_LENGTH,
                            declared_length.to_string(),
                        );
                    if let Some(content_range) = reply.content_range {
                        response = response
                            .header(http::header::CONTENT_RANGE, content_range);
                    }
                    let response = response.body(()).unwrap();
                    let end_stream = reply.body.is_empty()
                        && reply.reset_after_bytes.is_none();
                    let Ok(mut stream) =
                        respond.send_response(response, end_stream)
                    else {
                        return;
                    };
                    if let Some(reset_after_bytes) = reply.reset_after_bytes {
                        let count = reset_after_bytes.min(reply.body.len());
                        if count > 0
                            && stream
                                .send_data(
                                    Bytes::copy_from_slice(
                                        &reply.body[..count],
                                    ),
                                    false,
                                )
                                .is_err()
                        {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        stream.send_reset(h2::Reason::CANCEL);
                    } else if !reply.body.is_empty() {
                        let _ = stream.send_data(Bytes::from(reply.body), true);
                    }
                });
            }
        });
        let (sender, driver) = h2::client::handshake(client_io).await.unwrap();
        let driver = tokio::spawn(driver);
        let connection =
            SharedH2Connection::cache_for_test(route, sender).await;
        H2RangeFixture {
            connection,
            requests,
            server,
            driver,
        }
    }

    struct Http1RangeFixture {
        url: String,
        requests: Arc<Mutex<Vec<(u64, u64)>>>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn start_http1_range_fixture(
        data: Arc<Vec<u8>>,
    ) -> Http1RangeFixture {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server_data = data.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let data = server_data.clone();
                let requests = server_requests.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 1024];
                    loop {
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if request
                            .windows(4)
                            .any(|window| window == b"\r\n\r\n")
                        {
                            break;
                        }
                    }
                    let request =
                        String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let Some(range) = request
                        .lines()
                        .find_map(|line| line.strip_prefix("range: bytes="))
                    else {
                        return;
                    };
                    let Some((start, end)) = range.split_once('-') else {
                        return;
                    };
                    let (Ok(start), Ok(end)) =
                        (start.parse::<u64>(), end.parse::<u64>())
                    else {
                        return;
                    };
                    requests.lock().push((start, end));
                    let Some(body) = data.get(start as usize..=end as usize)
                    else {
                        return;
                    };
                    let headers = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        data.len(),
                    );
                    if stream.write_all(headers.as_bytes()).await.is_err()
                        || stream.write_all(body).await.is_err()
                    {
                        return;
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        Http1RangeFixture {
            url: format!("http://{address}/file"),
            requests,
            server,
        }
    }

    struct SlowRangeTransport {
        bytes: Arc<Vec<u8>>,
        calls: Mutex<Vec<u64>>,
        old_body_live: Arc<std::sync::atomic::AtomicBool>,
    }

    struct SlowBodyGuard(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for SlowBodyGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    #[async_trait]
    impl RangeTransport for SlowRangeTransport {
        async fn open(
            &self,
            start: u64,
            end: u64,
            attempt: usize,
        ) -> Result<RangeResponse<'_>, H2DownloadFailure> {
            assert!(
                !self.old_body_live.load(Ordering::Acquire),
                "old range body must close before recovery opens"
            );
            self.calls.lock().push(start);
            if attempt == 0 {
                self.old_body_live.store(true, Ordering::Release);
                let bytes = self.bytes.clone();
                let guard = SlowBodyGuard(self.old_body_live.clone());
                let body = futures::stream::unfold(
                    (start as usize, guard),
                    move |(offset, guard)| {
                        let bytes = bytes.clone();
                        async move {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            let next = (offset + 1024).min(end as usize + 1);
                            if next == offset {
                                return None;
                            }
                            Some((
                                Ok(Bytes::copy_from_slice(
                                    &bytes[offset..next],
                                )),
                                (next, guard),
                            ))
                        }
                    },
                );
                Ok(RangeResponse::fixture(Box::pin(body), true))
            } else {
                Ok(RangeResponse::fixture(
                    Box::pin(futures::stream::iter([Ok(
                        Bytes::copy_from_slice(
                            &self.bytes[start as usize..=end as usize],
                        ),
                    )])),
                    true,
                ))
            }
        }

        fn on_slow(
            &self,
            _identity: &RangeIdentity,
            _speed: u64,
            _remaining: u64,
            _region: std::ops::Range<u64>,
            _pressure: super::super::local_resources::Pressure,
        ) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn continuously_slow_range_recovers_only_unfinished_bytes_and_hash_matches()
     {
        let bytes = Arc::new(vec![42; 1024 * 1024]);
        let (request, route) = memory_request(&bytes);
        let transport = SlowRangeTransport {
            bytes: bytes.clone(),
            calls: Mutex::new(Vec::new()),
            old_body_live: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("final.bin");
        let part = directory.path().join("final.part");
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            super::super::native_slow::with_test_timing(
                Duration::from_millis(75),
                Duration::from_millis(150),
                run_download(
                    &transport,
                    &request,
                    &route,
                    &destination,
                    &part,
                    bytes.len() as u64,
                    1,
                    None,
                ),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, H2DownloadOutcome::Completed(_)));
        assert_eq!(tokio::fs::read(destination).await.unwrap(), *bytes);
        let calls = transport.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], 0);
        assert!(calls[1] > 0 && calls[1] < bytes.len() as u64);
    }

    struct UnsupportedRangeTransport;
    #[async_trait]
    impl RangeTransport for UnsupportedRangeTransport {
        async fn open(
            &self,
            _start: u64,
            _end: u64,
            _attempt: usize,
        ) -> Result<RangeResponse<'_>, H2DownloadFailure> {
            Err(range_response_failure(200))
        }
    }

    #[tokio::test]
    async fn unsupported_ranges_discard_sparse_output_and_checkpoints() {
        let directory = tempfile::tempdir().unwrap();
        let part = directory.path().join("range.part");
        let destination = directory.path().join("file");
        let request = DownloadRequest::new(
            "https://unsupported.invalid/file",
            fetch::ResourceClass::Other,
        )
        .with_integrity(fetch::Integrity::sha1("expected").with_size(8));
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: false,
            supports_range: true,
            proxy: fetch::ProxyPolicy::System,
        };
        let outcome = run_download(
            &UnsupportedRangeTransport,
            &request,
            &route,
            &destination,
            &part,
            8,
            2,
            None,
        )
        .await;
        assert!(matches!(
            outcome,
            H2DownloadOutcome::Fallback {
                failure: H2DownloadFailure::RangeUnsupported,
                preserve_partial: false
            }
        ));
        assert!(!part.exists());
        assert!(!super::super::range_journal::path(&part).exists());
        assert_eq!(
            range_response_failure(206),
            H2DownloadFailure::RangeUnsupported
        );
        assert_eq!(range_response_failure(503), H2DownloadFailure::Http);
    }

    #[test]
    fn splits_file_into_eight_contiguous_ranges() {
        let ranges = split_ranges(83, 8);
        assert_eq!(ranges.len(), 8);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, 82);
        assert!(
            ranges
                .windows(2)
                .all(|pair| pair[0].end + 1 == pair[1].start)
        );
    }

    #[test]
    fn splits_file_into_sixteen_contiguous_ranges() {
        let ranges = split_ranges(1024 * 1024 + 7, 16);
        assert_eq!(ranges.len(), 16);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, 1024 * 1024 + 6);
        assert!(
            ranges
                .windows(2)
                .all(|pair| pair[0].end + 1 == pair[1].start)
        );
    }

    struct MemoryTransport {
        data: Vec<u8>,
        fail_tail: bool,
        recover_on_http1: bool,
        calls: parking_lot::Mutex<Vec<(u64, usize)>>,
    }

    struct CancellingTransport {
        budget: Arc<tokio::sync::Semaphore>,
        cancellation: tokio_util::sync::CancellationToken,
    }

    #[async_trait]
    impl RangeTransport for CancellingTransport {
        async fn open(
            &self,
            _start: u64,
            _end: u64,
            _attempt: usize,
        ) -> Result<RangeResponse<'_>, H2DownloadFailure> {
            let permit = self.budget.clone().acquire_owned().await.unwrap();
            let cancellation = self.cancellation.clone();
            Ok(RangeResponse::fixture(
                Box::pin(futures::stream::unfold(permit, move |permit| {
                    let cancellation = cancellation.clone();
                    async move {
                        cancellation.cancel();
                        futures::future::pending::<()>().await;
                        Some((Ok(Bytes::new()), permit))
                    }
                })),
                true,
            ))
        }
    }

    #[tokio::test]
    async fn memory_recovery_cancellation_releases_active_transfer_weights() {
        let (mut request, route) =
            memory_request(b"abcdefghijklmnopqrstuvwxyz0123456789ABCD");
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("archive");
        let part = directory.path().join("archive.part");
        let cancellation = tokio_util::sync::CancellationToken::new();
        request.cancellation = Some(cancellation.clone());
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let transport = CancellingTransport {
            budget: budget.clone(),
            cancellation,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_download(
                &transport,
                &request,
                &route,
                &destination,
                &part,
                40,
                2,
                None,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, H2DownloadOutcome::Canceled));
        assert_eq!(budget.available_permits(), 1);
        assert!(part.exists());
        assert!(super::super::range_journal::path(&part).exists());
    }

    #[tokio::test]
    async fn memory_recovery_cancellation_keeps_prior_checkpoints() {
        let data = b"abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_vec();
        let (mut request, route) = memory_request(&data);
        let directory = tempfile::tempdir().unwrap();
        let part = directory.path().join("archive.part");
        let destination = directory.path().join("archive");
        tokio::fs::write(&part, &data).await.unwrap();
        let journal =
            RangeJournal::new(&part, &request.integrity, data.len() as u64, 2);
        journal
            .checkpoint(0, 20, &sha2::Sha256::new_with_prefix(&data[..20]))
            .await
            .unwrap();
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        request.cancellation = Some(cancellation);
        let transport = MemoryTransport {
            data,
            fail_tail: false,
            recover_on_http1: false,
            calls: Default::default(),
        };
        assert!(matches!(
            run_download(
                &transport,
                &request,
                &route,
                &destination,
                &part,
                40,
                2,
                None
            )
            .await,
            H2DownloadOutcome::Canceled
        ));
        assert!(transport.calls.lock().is_empty());
        assert!(part.exists());
        assert!(super::super::range_journal::path(&part).exists());
    }

    #[async_trait]
    impl RangeTransport for MemoryTransport {
        async fn open(
            &self,
            start: u64,
            end: u64,
            attempt: usize,
        ) -> Result<RangeResponse<'_>, H2DownloadFailure> {
            self.calls.lock().push((start, attempt));
            if start >= self.data.len() as u64 / 2 && self.fail_tail {
                if self.recover_on_http1 && attempt >= 2 {
                    return Ok(RangeResponse::fixture(
                        Box::pin(futures::stream::iter([Ok(
                            Bytes::copy_from_slice(
                                &self.data[start as usize..=end as usize],
                            ),
                        )])),
                        false,
                    ));
                }
                if attempt == 0 {
                    return Ok(RangeResponse::fixture(
                        Box::pin(futures::stream::iter([
                            Ok(Bytes::copy_from_slice(
                                &self.data[start as usize..start as usize + 3],
                            )),
                            Err(H2DownloadFailure::Protocol),
                        ])),
                        true,
                    ));
                }
                return Err(H2DownloadFailure::Protocol);
            }
            Ok(RangeResponse::fixture(
                Box::pin(futures::stream::iter([Ok(Bytes::copy_from_slice(
                    &self.data[start as usize..=end as usize],
                ))])),
                attempt < 2,
            ))
        }
    }

    fn memory_request(data: &[u8]) -> (DownloadRequest, DownloadRoute) {
        let request = DownloadRequest::new(
            "https://range-memory.invalid/archive",
            fetch::ResourceClass::Modpack,
        )
        .with_integrity(
            fetch::Integrity::sha1(Sha1::from(data).hexdigest())
                .with_size(data.len() as u64),
        );
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        (request, route)
    }

    #[tokio::test]
    async fn memory_recovery_retries_offsets_and_switches_only_failed_range() {
        let data = b"abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_vec();
        let (request, route) = memory_request(&data);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("archive");
        let part = directory.path().join("archive.part");
        let transport = MemoryTransport {
            data: data.clone(),
            fail_tail: true,
            recover_on_http1: true,
            calls: Default::default(),
        };
        let result = run_download(
            &transport,
            &request,
            &route,
            &destination,
            &part,
            data.len() as u64,
            2,
            None,
        )
        .await;
        assert!(matches!(result, H2DownloadOutcome::Completed(_)));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), data);
        let calls = transport.calls.lock();
        assert_eq!(calls.iter().filter(|(start, _)| *start == 0).count(), 1);
        assert!(calls.contains(&(23, 2)));
        assert!(!part.exists());
        assert!(!super::super::range_journal::path(&part).exists());
    }

    #[tokio::test]
    async fn memory_recovery_preserves_completed_ranges_for_next_attempt() {
        let data = b"abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_vec();
        let (request, route) = memory_request(&data);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("archive");
        let part = directory.path().join("archive.part");
        let failed = MemoryTransport {
            data: data.clone(),
            fail_tail: true,
            recover_on_http1: false,
            calls: Default::default(),
        };
        let result = run_download(
            &failed,
            &request,
            &route,
            &destination,
            &part,
            data.len() as u64,
            2,
            None,
        )
        .await;
        assert!(matches!(
            result,
            H2DownloadOutcome::Fallback {
                preserve_partial: true,
                ..
            }
        ));
        assert!(part.exists());
        let journal =
            RangeJournal::load(&part, &request.integrity, data.len() as u64)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(journal.ranges().await[0].written, 20);
        let recovered = MemoryTransport {
            data: data.clone(),
            fail_tail: false,
            recover_on_http1: false,
            calls: Default::default(),
        };
        assert!(matches!(
            run_download(
                &recovered,
                &request,
                &route,
                &destination,
                &part,
                data.len() as u64,
                2,
                Some(journal)
            )
            .await,
            H2DownloadOutcome::Completed(_)
        ));
        assert_eq!(*recovered.calls.lock(), vec![(23, 0)]);
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), data);
    }

    #[tokio::test]
    async fn memory_recovery_rejects_bad_final_digest_and_keeps_live_destination()
     {
        let data = b"abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_vec();
        let (request, route) = memory_request(&data);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("archive");
        let part = directory.path().join("archive.part");
        tokio::fs::write(&destination, b"previous installation")
            .await
            .unwrap();
        let transport = MemoryTransport {
            data: vec![0; data.len()],
            fail_tail: false,
            recover_on_http1: false,
            calls: Default::default(),
        };
        assert!(matches!(
            run_download(
                &transport,
                &request,
                &route,
                &destination,
                &part,
                data.len() as u64,
                2,
                None
            )
            .await,
            H2DownloadOutcome::Fallback {
                preserve_partial: false,
                ..
            }
        ));
        assert!(!part.exists());
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"previous installation"
        );
    }

    #[tokio::test]
    async fn actual_h2_rejects_incompatible_range_responses_and_preserves_destination()
     {
        let data = b"verified h2 range payload".to_vec();
        let size = data.len() as u64;
        let cases = vec![
            (
                "ignored-range",
                StatusCode::OK,
                None,
                data.clone(),
                H2DownloadFailure::RangeUnsupported,
            ),
            (
                "range-416",
                StatusCode::RANGE_NOT_SATISFIABLE,
                Some(format!("bytes */{size}")),
                Vec::new(),
                H2DownloadFailure::RangeUnsupported,
            ),
            (
                "missing-content-range",
                StatusCode::PARTIAL_CONTENT,
                None,
                Vec::new(),
                H2DownloadFailure::RangeUnsupported,
            ),
            (
                "wrong-content-range",
                StatusCode::PARTIAL_CONTENT,
                Some(format!("bytes 1-{}/{size}", size - 1)),
                Vec::new(),
                H2DownloadFailure::RangeUnsupported,
            ),
        ];

        for (name, status, content_range, body, expected_failure) in cases {
            let url = format!("https://h2-{name}.test/file");
            let request =
                DownloadRequest::new(&url, fetch::ResourceClass::Other)
                    .with_integrity(
                        fetch::Integrity::sha1(Sha1::from(&data).hexdigest())
                            .with_size(size),
                    );
            let route = DownloadRoute {
                url: request.url.clone(),
                source: fetch::DownloadRouteSource::Official,
                is_mirror: false,
                allow_sensitive_headers: true,
                supports_range: true,
                proxy: fetch::ProxyPolicy::Direct,
            };
            let reply_body = body.clone();
            let reply_range = content_range.clone();
            let fixture =
                start_h2_range_fixture(&route, move |_start, _end, _index| {
                    H2RangeReply {
                        status,
                        content_range: reply_range.clone(),
                        body: reply_body.clone(),
                        reset_after_bytes: None,
                    }
                })
                .await;
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("installed-pack.zip");
            let part = directory.path().join("installed-pack.zip.part");
            tokio::fs::write(&destination, b"previous installation")
                .await
                .unwrap();
            let uri = route.url.parse().unwrap();

            let outcome = download(
                &fixture.connection,
                &uri,
                &request,
                &route,
                &destination,
                &part,
                size,
                1,
                None,
            )
            .await;

            assert!(matches!(
                outcome,
                H2DownloadOutcome::Fallback {
                    failure,
                    preserve_partial: false
                } if failure == expected_failure
            ));
            assert!(!part.exists(), "case {name} left a sparse part file");
            assert!(
                !super::super::range_journal::path(&part).exists(),
                "case {name} left a range journal"
            );
            assert_eq!(
                tokio::fs::read(&destination).await.unwrap(),
                b"previous installation",
                "case {name} replaced the installed destination"
            );
            assert_eq!(
                *fixture.requests.lock(),
                vec![(0, size - 1)],
                "case {name} did not issue one bounded Range request"
            );
            fixture.driver.abort();
            fixture.server.abort();
        }
    }

    #[tokio::test]
    async fn actual_h2_integrity_failure_discards_partial_and_keeps_destination()
     {
        let expected = b"the expected archive contents".to_vec();
        let corrupt = vec![0x5a; expected.len()];
        let size = expected.len() as u64;
        let url = "https://h2-range-corrupt.test/file".to_string();
        let request = DownloadRequest::new(&url, fetch::ResourceClass::Modpack)
            .with_integrity(
                fetch::Integrity::sha1(Sha1::from(&expected).hexdigest())
                    .with_size(size),
            );
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        let response_body = Arc::new(corrupt);
        let fixture =
            start_h2_range_fixture(&route, move |start, end, _| H2RangeReply {
                status: StatusCode::PARTIAL_CONTENT,
                content_range: Some(format!("bytes {start}-{end}/{size}")),
                body: response_body[start as usize..=end as usize].to_vec(),
                reset_after_bytes: None,
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("pack.mrpack");
        let part = directory.path().join("pack.mrpack.part");
        tokio::fs::write(&destination, b"previous valid pack")
            .await
            .unwrap();
        let uri = route.url.parse().unwrap();

        let outcome = download(
            &fixture.connection,
            &uri,
            &request,
            &route,
            &destination,
            &part,
            size,
            1,
            None,
        )
        .await;

        assert!(matches!(
            outcome,
            H2DownloadOutcome::Fallback {
                failure: H2DownloadFailure::Integrity,
                preserve_partial: false
            }
        ));
        assert!(!part.exists());
        assert!(!super::super::range_journal::path(&part).exists());
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"previous valid pack"
        );
        fixture.driver.abort();
        fixture.server.abort();
    }

    #[tokio::test]
    async fn actual_h2_stream_resets_keep_checkpoints_and_resume_only_the_tail()
    {
        let data = Arc::new(
            (0..256 * 1024)
                .map(|index| (index % 239) as u8)
                .collect::<Vec<_>>(),
        );
        let http1_fixture = start_http1_range_fixture(data.clone()).await;
        let url = http1_fixture.url.clone();
        let request = DownloadRequest::new(&url, fetch::ResourceClass::Other)
            .with_integrity(
                fetch::Integrity::sha1(Sha1::from(&data[..]).hexdigest())
                    .with_size(data.len() as u64),
            );
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        let reset_prefix = 16 * 1024;
        let h2_data = data.clone();
        let fixture =
            start_h2_range_fixture(&route, move |start, end, index| {
                H2RangeReply {
                    status: StatusCode::PARTIAL_CONTENT,
                    content_range: Some(format!(
                        "bytes {start}-{end}/{}",
                        h2_data.len()
                    )),
                    body: h2_data[start as usize..=end as usize].to_vec(),
                    reset_after_bytes: (index < H2_RANGE_RETRY_ATTEMPTS)
                        .then_some(reset_prefix),
                }
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("resumed.bin");
        let part = directory.path().join("resumed.bin.part");
        tokio::fs::write(&destination, b"previous valid file")
            .await
            .unwrap();
        let uri = route.url.parse().unwrap();

        let interrupted = tokio::time::timeout(
            Duration::from_secs(10),
            download(
                &fixture.connection,
                &uri,
                &request,
                &route,
                &destination,
                &part,
                data.len() as u64,
                1,
                None,
            ),
        )
        .await
        .expect("repeated H2 stream resets should reach the fallback boundary");

        assert!(matches!(
            interrupted,
            H2DownloadOutcome::Fallback {
                failure: H2DownloadFailure::StreamReset,
                preserve_partial: true
            }
        ));
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"previous valid file"
        );
        let journal = RangeJournal::load(
            &part,
            &request.integrity,
            data.len() as u64,
        )
        .await
        .unwrap()
        .expect("recoverable Range resets must preserve a validated journal");
        let checkpoints = journal.ranges().await;
        assert_eq!(
            checkpoints[0].written,
            reset_prefix as u64 * H2_RANGE_RETRY_ATTEMPTS as u64
        );

        let semaphore = fetch::FetchSemaphore(tokio::sync::Semaphore::new(8));
        let resumed =
            resume_http1(&request, &route, &destination, &part, &semaphore)
                .await
                .unwrap()
                .unwrap();

        assert!(matches!(resumed, H2DownloadOutcome::Completed(_)));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), *data);
        let h2_ranges = fixture.requests.lock().clone();
        let http1_ranges = http1_fixture.requests.lock().clone();
        let mut ranges = h2_ranges;
        ranges.extend(http1_ranges);
        assert_eq!(ranges.len(), H2_RANGE_RETRY_ATTEMPTS + 1);
        assert_eq!(
            ranges.iter().map(|(start, _)| *start).collect::<Vec<_>>(),
            (0..=H2_RANGE_RETRY_ATTEMPTS)
                .map(|attempt| reset_prefix as u64 * attempt as u64)
                .collect::<Vec<_>>()
        );
        assert!(!part.exists());
        assert!(!super::super::range_journal::path(&part).exists());
        fixture.driver.abort();
        fixture.server.abort();
        http1_fixture.server.abort();
    }

    #[tokio::test]
    async fn h2_resume_uses_saved_checkpoints_without_refetching_completed_prefix()
     {
        let data = Arc::new(
            (0..512 * 1024)
                .map(|index| (index % 239) as u8)
                .collect::<Vec<_>>(),
        );
        let url = "https://h2-range-resume.test/file".to_string();
        let request = DownloadRequest::new(&url, fetch::ResourceClass::Modpack)
            .with_integrity(
                fetch::Integrity::sha1(Sha1::from(&data[..]).hexdigest())
                    .with_size(data.len() as u64),
            );
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        let h2_data = data.clone();
        let fixture =
            start_h2_range_fixture(&route, move |start, end, _| H2RangeReply {
                status: StatusCode::PARTIAL_CONTENT,
                content_range: Some(format!(
                    "bytes {start}-{end}/{}",
                    h2_data.len()
                )),
                body: h2_data[start as usize..=end as usize].to_vec(),
                reset_after_bytes: None,
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("resumed-pack.mrpack");
        let part = directory.path().join("resumed-pack.mrpack.part");
        tokio::fs::write(&part, &data[..]).await.unwrap();
        let prefix = 16 * 1024;
        let journal =
            RangeJournal::new(&part, &request.integrity, data.len() as u64, 2);
        journal
            .checkpoint(
                0,
                prefix as u64,
                &sha2::Sha256::new_with_prefix(&data[..prefix]),
            )
            .await
            .unwrap();
        journal.persist().await.unwrap();
        let semaphore = fetch::FetchSemaphore(tokio::sync::Semaphore::new(8));

        let outcome =
            resume_http1(&request, &route, &destination, &part, &semaphore)
                .await
                .unwrap()
                .unwrap();

        assert!(matches!(outcome, H2DownloadOutcome::Completed(_)));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), *data);
        assert!(
            fixture
                .requests
                .lock()
                .iter()
                .any(|(start, _)| *start == prefix as u64)
        );
        assert!(fixture.requests.lock().iter().all(|(start, _)| *start != 0));
        assert!(!part.exists());
        assert!(!super::super::range_journal::path(&part).exists());
        fixture.driver.abort();
        fixture.server.abort();
    }

    #[tokio::test]
    async fn downloads_one_file_over_eight_h2_range_streams() {
        let data = Arc::new(
            (0..2 * 1024 * 1024)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let request_count = Arc::new(AtomicU64::new(0));
        let (client_io, server_io) = tokio::io::duplex(256 * 1024);
        let server_data = Arc::clone(&data);
        let server_requests = Arc::clone(&request_count);
        let server = tokio::spawn(async move {
            let mut connection =
                h2::server::handshake(server_io).await.unwrap();
            while let Some(result) = connection.accept().await {
                let (request, mut respond) = result.unwrap();
                let data = Arc::clone(&server_data);
                let requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    requests.fetch_add(1, Ordering::Relaxed);
                    let value = request.headers()[RANGE].to_str().unwrap();
                    let value = value.strip_prefix("bytes=").unwrap();
                    let (start, end) = value.split_once('-').unwrap();
                    let start = start.parse::<usize>().unwrap();
                    let end = end.parse::<usize>().unwrap();
                    let response = Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/{}", data.len()),
                        )
                        .body(())
                        .unwrap();
                    let mut stream =
                        respond.send_response(response, false).unwrap();
                    let mut offset = start;
                    while offset <= end {
                        let wanted = (end - offset + 1).min(16 * 1024);
                        stream.reserve_capacity(wanted);
                        let capacity = poll_fn(|cx| stream.poll_capacity(cx))
                            .await
                            .unwrap()
                            .unwrap();
                        let length = capacity.min(wanted);
                        let finished = offset + length > end;
                        stream
                            .send_data(
                                Bytes::copy_from_slice(
                                    &data[offset..offset + length],
                                ),
                                finished,
                            )
                            .unwrap();
                        offset += length;
                    }
                });
            }
        });

        let mut builder = h2::client::Builder::new();
        builder
            .initial_window_size(1024 * 1024)
            .initial_connection_window_size(64 * 1024 * 1024);
        let (sender, mut driver) =
            builder.handshake::<_, Bytes>(client_io).await.unwrap();
        driver.set_target_window_size(64 * 1024 * 1024);
        let client_driver = tokio::spawn(driver);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("pack.mrpack");
        let part_path = directory.path().join("pack.mrpack.part");
        let hash = Sha1::from(&data[..]).hexdigest();
        let request = DownloadRequest::new(
            "https://h2-range.test/pack.mrpack",
            fetch::ResourceClass::Modpack,
        )
        .with_integrity(
            fetch::Integrity::sha1(hash).with_size(data.len() as u64),
        );
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        let connection =
            SharedH2Connection::cache_for_test(&route, sender).await;
        let uri = route.url.parse().unwrap();

        let result = download(
            &connection,
            &uri,
            &request,
            &route,
            &destination,
            &part_path,
            data.len() as u64,
            8,
            None,
        )
        .await;

        assert!(matches!(result, H2DownloadOutcome::Completed(_)));
        assert_eq!(request_count.load(Ordering::Relaxed), 8);
        assert_eq!(tokio::fs::read(destination).await.unwrap(), *data);
        client_driver.abort();
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_stops_all_ranges_and_removes_partial_output() {
        let request_count = Arc::new(AtomicU64::new(0));
        let (client_io, server_io) = tokio::io::duplex(256 * 1024);
        let server_requests = Arc::clone(&request_count);
        let server = tokio::spawn(async move {
            let mut connection =
                h2::server::handshake(server_io).await.unwrap();
            while let Some(result) = connection.accept().await {
                let (request, mut respond) = result.unwrap();
                let requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    requests.fetch_add(1, Ordering::Relaxed);
                    let value = request.headers()[RANGE].to_str().unwrap();
                    let value = value.strip_prefix("bytes=").unwrap();
                    let (start, end) = value.split_once('-').unwrap();
                    let response = Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/2097152"),
                        )
                        .body(())
                        .unwrap();
                    let _stream =
                        respond.send_response(response, false).unwrap();
                    futures::future::pending::<()>().await;
                });
            }
        });

        let (sender, driver) = h2::client::handshake(client_io).await.unwrap();
        let client_driver = tokio::spawn(driver);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("pack.zip");
        let part_path = directory.path().join("pack.zip.part");
        tokio::fs::write(&destination, b"previous installed pack")
            .await
            .unwrap();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut request = DownloadRequest::new(
            "https://h2-range-cancel.test/pack.zip",
            fetch::ResourceClass::CurseForge,
        )
        .with_integrity(fetch::Integrity::default().with_size(2 * 1024 * 1024));
        request.cancellation = Some(cancellation.clone());
        let route = DownloadRoute {
            url: request.url.clone(),
            source: fetch::DownloadRouteSource::Official,
            is_mirror: false,
            allow_sensitive_headers: true,
            supports_range: true,
            proxy: fetch::ProxyPolicy::Direct,
        };
        let connection =
            SharedH2Connection::cache_for_test(&route, sender).await;
        let uri = route.url.parse().unwrap();
        let part_for_task = part_path.clone();
        let destination_for_task = destination.clone();
        let download_task = tokio::spawn(async move {
            download(
                &connection,
                &uri,
                &request,
                &route,
                &destination_for_task,
                &part_for_task,
                2 * 1024 * 1024,
                8,
                None,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while request_count.load(Ordering::Relaxed) < 8 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        cancellation.cancel();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            download_task,
        )
        .await
        .unwrap()
        .unwrap();

        assert!(matches!(outcome, H2DownloadOutcome::Canceled));
        assert!(!part_path.exists());
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"previous installed pack"
        );
        client_driver.abort();
        server.abort();
    }
}
