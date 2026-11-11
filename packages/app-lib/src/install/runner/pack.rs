use super::*;

/// Upper bound for scanning a dropped archive for nested packs.
///
/// The scan is pure local I/O, so anything longer means a corrupt central
/// directory, an absurd compression ratio, or a file another process is still
/// holding — all of which must surface as an error rather than an install that
/// sits on the parsing phase forever.
const PACK_ARCHIVE_SCAN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

/// Largest total payload the nested-archive scan may write to scratch.
///
/// Reuses the limit the rest of local pack extraction already enforces
/// (`archive_util::EXTRACTION_SIZE_LIMIT`) rather than inventing a second,
/// different ceiling: a nested archive is extracted by the same code path once
/// this scan hands it over, so a scan that accepted more than that path allows
/// would only defer the rejection. 8 GiB is generous for a pack that has to be
/// downloaded and installed from the same disk, and it bounds the scan without
/// changing any legitimate pack that already installs today.
///
/// This is a size bound, not a disk-space guarantee: within the cap the scan
/// can still fill the cache volume, so a free-space check remains the caller's
/// responsibility and exhaustion inside the cap is still possible.
const PACK_ARCHIVE_SCAN_TOTAL_BYTES: u64 =
    crate::api::pack::archive_util::EXTRACTION_SIZE_LIMIT;

/// Copy chunk used while extracting a nested archive.
const PACK_ARCHIVE_SCAN_CHUNK_BYTES: usize = 256 * 1024;

/// Returned when the cooperative cancellation flag stopped the scan early.
const PACK_ARCHIVE_SCAN_CANCELLED: &str = "Modpack archive scan was canceled";

#[cfg(test)]
thread_local! {
    /// Test seam: invoked after each nested archive member is fully written,
    /// so a test can raise the cancellation flag mid-scan. A pre-raised flag
    /// would only exercise the entry-loop guard, not a scan that is actually
    /// interrupted partway through.
    static AFTER_ENTRY: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn notify_after_entry() {
    AFTER_ENTRY.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook();
        }
    });
}

/// Walks `archive` and writes every nested `.zip`/`.mrpack` member to `scratch`.
///
/// Blocking by design: the `zip` crate is synchronous, so this must only ever
/// run inside `spawn_blocking`. `cancel` is the only way to stop it early —
/// Tokio cannot cancel a blocking task once it has started — so the copy loop
/// polls the flag between chunks and the entry loop polls it between entries.
/// `scratch` is removed by this function on every exit path, because only this
/// thread knows when its file handles are actually closed; on Windows the async
/// caller cannot delete files it may still hold open.
fn scan_sub_archives(
    archive: &std::path::Path,
    scratch: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> crate::Result<Vec<PathBuf>> {
    scan_sub_archives_within(
        archive,
        scratch,
        cancel,
        PACK_ARCHIVE_SCAN_TOTAL_BYTES,
    )
}

/// `scan_sub_archives` with an explicit extraction budget.
///
/// Split out only so a test can drive the budget branch with a few bytes
/// instead of needing an 8 GiB archive. Production always calls
/// `scan_sub_archives`, which supplies the shared `EXTRACTION_SIZE_LIMIT`; the
/// branch under test is the same one either way.
fn scan_sub_archives_within(
    archive: &std::path::Path,
    scratch: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
    total_budget: u64,
) -> crate::Result<Vec<PathBuf>> {
    let result =
        scan_sub_archives_inner(archive, scratch, cancel, total_budget);
    // Final cancellation check, after the inner walk has returned. Without it a
    // cancel that lands between the last in-loop check and the return would be
    // missed and the worker would report success, so the caller would keep the
    // paths while the directory is already being deleted. Treating it as an
    // error also makes `result.is_err()` true below, so the removal runs.
    let result = if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(crate::ErrorKind::InputError(
            PACK_ARCHIVE_SCAN_CANCELLED.to_string(),
        )
        .into())
    } else {
        result
    };
    // Remove on failure, and also on success if cancellation was requested:
    // that combination means the caller already timed out or was dropped and is
    // discarding these paths, so nothing will ever read them. Leaving it to the
    // caller is not enough there — its removal races handles this thread holds.
    if result.is_err() || cancel.load(std::sync::atomic::Ordering::Relaxed) {
        // Handles are dropped by the time we get here, so this is safe even on
        // Windows, where deleting an open file would fail.
        let _ = std::fs::remove_dir_all(scratch);
    }
    result
}

fn scan_sub_archives_inner(
    archive: &std::path::Path,
    scratch: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
    total_budget: u64,
) -> crate::Result<Vec<PathBuf>> {
    use std::io::Read;
    use std::sync::atomic::Ordering;

    let file = std::fs::File::open(archive)?;
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return Err(crate::ErrorKind::InputError(
            "Unrecognized modpack format: no known pack manifest was found in the archive"
                .to_string(),
        )
        .into());
    };

    let mut sub_archive_paths = Vec::new();
    let mut extracted_bytes = 0_u64;
    let mut buffer = vec![0_u8; PACK_ARCHIVE_SCAN_CHUNK_BYTES];
    for index in 0..zip.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ErrorKind::InputError(
                PACK_ARCHIVE_SCAN_CANCELLED.to_string(),
            )
            .into());
        }

        let lower_name = {
            let entry = zip
                .by_index_raw(index)
                .map_err(|error| ErrorKind::OtherError(error.to_string()))?;
            let name = crate::api::pack::detect::decode_zip_entry_name(
                entry.name_raw(),
            );
            name.to_lowercase()
        };

        if !lower_name.ends_with(".zip") && !lower_name.ends_with(".mrpack") {
            continue;
        }

        let mut entry = zip
            .by_index(index)
            .map_err(|error| ErrorKind::OtherError(error.to_string()))?;
        // The declared size is the budget for this entry; a member that lies
        // about it is cut off rather than allowed to inflate forever.
        let declared = entry.size();
        let remaining_budget = total_budget.saturating_sub(extracted_bytes);
        // Reject before creating the file or reading a single byte. Extracting
        // whatever remains of the budget and only then erroring would write
        // gigabytes of a member we already know cannot fit, and would still
        // have to delete them.
        if declared > remaining_budget {
            return Err(crate::ErrorKind::InputError(format!(
                "Modpack archive scan exceeded its {total_budget}-byte extraction budget"
            ))
            .into());
        }

        // Re-check cancellation immediately before touching the filesystem. The
        // entry-loop check above is not enough: the flag can be raised while
        // this entry is being inspected, and creating directories or a file
        // after that point is exactly what recreates a scratch tree the owner
        // is already deleting.
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ErrorKind::InputError(
                PACK_ARCHIVE_SCAN_CANCELLED.to_string(),
            )
            .into());
        }
        let sub_path = scratch.join(entry.mangled_name());
        if let Some(parent) = sub_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Also checked after `create_dir_all` and before the file is created: a
        // cancel that landed during directory creation must not leave a new
        // empty directory behind for the deletion to miss.
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ErrorKind::InputError(
                PACK_ARCHIVE_SCAN_CANCELLED.to_string(),
            )
            .into());
        }
        let mut output = std::fs::File::create(&sub_path)?;
        let mut written = 0_u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::ErrorKind::InputError(
                    PACK_ARCHIVE_SCAN_CANCELLED.to_string(),
                )
                .into());
            }
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > declared {
                return Err(crate::ErrorKind::InputError(format!(
                    "Modpack archive member declares {declared} bytes but contains more"
                ))
                .into());
            }
            std::io::Write::write_all(&mut output, &buffer[..read])?;
        }
        if written < declared {
            // The archive ended before the entry delivered its declared size:
            // a truncated or corrupt member. Reject it here rather than hand a
            // short file to the recursive pass, which would parse garbage as a
            // nested pack. The total-budget case was rejected before this
            // member was opened, so a short read here is genuinely truncation.
            return Err(crate::ErrorKind::InputError(format!(
                "Modpack archive member declares {declared} bytes but delivered {written}"
            ))
            .into());
        }
        extracted_bytes += written;
        drop(output);
        sub_archive_paths.push(sub_path);
        #[cfg(test)]
        notify_after_entry();
    }
    Ok(sub_archive_paths)
}

/// Removes an import scratch directory on drop.
///
/// Covers the recursive nested-archive scan in `install_local_pack_file_recursive`
/// only. Other scratch directories in this file are created and cleaned on their
/// own paths and are not managed by this guard.
///
/// The scan and the recursive installer return from many places, and every one
/// of them used to have to remember the deletion. A guard makes the cleanup a
/// property of scope instead: a failed progress emit, a `?` on any later step,
/// or a panic all still remove the extracted copy of the user's archive.
///
/// Deletion is synchronous because `Drop` cannot await. That is acceptable here
/// — the directory only holds files this task just wrote — and it also solves
/// the Windows case, where the removal must not race an open handle.
///
/// While a blocking scan is in flight the primary owner is that worker: only it
/// knows when its file handles are closed. `defer_to_worker` records that, and
/// the flag is carried here so the deferred `Drop` can still cancel and attempt
/// a best-effort removal — see `Drop` for why that is needed and why it cannot
/// be a guarantee.
struct ScratchDirGuard {
    path: Option<PathBuf>,
    deferred_to_worker: bool,
    /// The scan's cooperative flag, once one exists. Lets a deferred drop stop
    /// a worker that has not yet noticed the parent future disappear.
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl ScratchDirGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            deferred_to_worker: false,
            cancel: None,
        }
    }

    /// Hands primary removal to a blocking worker, keeping the flag so the
    /// deferred `Drop` can still cancel it and retry the delete.
    fn defer_to_worker(
        &mut self,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.deferred_to_worker = true;
        self.cancel = Some(cancel);
    }

    /// Takes removal back once the worker has provably finished.
    fn reclaim_after_worker(&mut self) {
        self.deferred_to_worker = false;
        self.cancel = None;
    }

    /// Stops this guard from deleting anything, for a path where the worker has
    /// already removed the directory or is still responsible for it.
    fn disarm_missing(&mut self) {
        self.deferred_to_worker = true;
        // Also drop the stored path so even a later `reclaim_after_worker`
        // cannot resurrect the deletion.
        self.path = None;
        self.cancel = None;
    }
}

impl Drop for ScratchDirGuard {
    fn drop(&mut self) {
        if !self.deferred_to_worker {
            if let Some(path) = self.path.take() {
                let _ = std::fs::remove_dir_all(path);
            }
            return;
        }

        // Deferred: the worker is the primary owner, but this drop also means
        // the operation is being abandoned (a `?`, a cancel, an unwind, or the
        // parent future being dropped). Raise the flag first so a worker that
        // has not yet re-read it stops before its next entry or chunk.
        //
        // This guard is declared before `ScanCancellationGuard`, so it drops
        // *after* it and cannot rely on that guard having set the flag already;
        // hence the store here rather than an assumption about ordering. Both
        // stores are idempotent.
        if let Some(cancel) = &self.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // Best-effort delete. This closes the window where the worker already
        // passed its final `cancel.load()` and returned success, so no one else
        // would ever remove the directory. It cannot be a guarantee: if the
        // worker still holds a handle, Windows refuses and the attempt is
        // dropped — the worker's own cleanup then owns it. And if the worker is
        // parked inside a synchronous `read`, it may write again before noticing
        // the flag; that residual is documented at the call site.
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Raises a scan's cooperative cancellation flag when dropped.
///
/// Tokio cannot abort a `spawn_blocking` task, so the flag is the only way to
/// stop the worker early. The timeout arm is not the only way out of the scan:
/// a user cancel, a cancelled install job, or any unwind while the scan future
/// is in flight drops this operation too. Without this guard those paths leave
/// a worker inflating an archive that nobody is waiting for, still writing into
/// a scratch directory the installer has abandoned.
///
/// On a normal finish the worker has already been joined, so `disarm` is called
/// and dropping is a no-op — setting the flag then would be harmless but the
/// explicit disarm keeps the intent obvious.
struct ScanCancellationGuard {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    active: bool,
}

impl ScanCancellationGuard {
    fn new(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self { flag, active: true }
    }

    /// The worker has stopped, so there is nothing left to cancel.
    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for ScanCancellationGuard {
    fn drop(&mut self) {
        if self.active {
            self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

pub(super) async fn install_pack(
    job_id: Uuid,
    job_state: &mut InstallJobState,
    location: CreatePackLocation,
    instance_id: String,
    reason: DownloadReason,
    capture_game_options: bool,
) -> crate::Result<InstallExecutionOutcome<()>> {
    let reporter = InstallProgressReporter::new(job_id, job_state.clone());
    reporter
        .update(
            InstallPhaseId::DownloadingPackFile,
            None,
            modpack_details(&location),
        )
        .await?;

    let create_pack = match location {
        CreatePackLocation::FromVersionId {
            project_id,
            version_id,
            title,
            icon_url,
        } => {
            reporter
                .set_context(
                    InstallErrorContext::new("download modpack file")
                        .project_id(project_id.clone())
                        .version_id(version_id.clone())
                        .build(),
                )
                .await?;
            generate_pack_from_version_id_with_reporter(
                project_id,
                version_id,
                title,
                icon_url,
                instance_id.clone(),
                reason,
                reporter.clone(),
            )
            .await?
        }
        CreatePackLocation::FromFile { path } => {
            reporter
                .set_context(
                    InstallErrorContext::new("read local modpack file")
                        .source_path(path.display().to_string())
                        .build(),
                )
                .await?;
            match crate::api::pack::detect::detect_local_pack(&path).await {
                Ok(detected) => {
                    if detected.format
                        != crate::api::pack::detect::LocalPackFormat::Mrpack
                    {
                        // Non-mrpack format — dispatch to format-specific
                        // installer via install_local_pack_file.
                        return install_local_pack_file(
                            detected,
                            path,
                            instance_id,
                            reporter,
                            capture_game_options,
                        )
                        .await;
                    }
                    // Mrpack — fall through to standard mrpack install.
                    generate_pack_from_file(path, instance_id.clone()).await?
                }
                Err(detect_error) => {
                    // No format recognised — try recursive extraction
                    // (3-level deep search for sub-archives, bundled
                    // packs, etc.) before giving up.
                    tracing::debug!(
                        "Local pack format detection failed, trying recursive extraction: {detect_error}"
                    );
                    return install_local_pack_file_recursive(
                        path,
                        instance_id,
                        reporter,
                        0,
                        3,
                        capture_game_options,
                    )
                    .await;
                }
            }
        }
    };

    let outcome = install_zipped_mrpack_files_with_reporter(
        create_pack,
        false,
        reason,
        reporter,
        capture_game_options,
    )
    .await?;
    Ok(match outcome {
        MrpackInstallOutcome::Completed => {
            InstallExecutionOutcome::Completed(())
        }
        MrpackInstallOutcome::WaitingForUser(reason) => {
            InstallExecutionOutcome::WaitingForUser(reason)
        }
    })
}

/// Recursively tries to detect and install a modpack, up to max_depth levels.
#[async_recursion::async_recursion]
async fn install_local_pack_file_recursive(
    path: PathBuf,
    instance_id: String,
    reporter: InstallProgressReporter,
    current_depth: usize,
    max_depth: usize,
    capture_game_options: bool,
) -> crate::Result<InstallExecutionOutcome<()>> {
    // First try standard detection - this will already include our InstanceFolder fallback
    if let Ok(detected) =
        crate::api::pack::detect::detect_local_pack(&path).await
    {
        // If it's a standard format (including InstanceFolder), just install it
        return install_local_pack_file(
            detected,
            path,
            instance_id,
            reporter,
            capture_game_options,
        )
        .await;
    }

    // If standard detection failed and we're not at max depth, try to look for
    // sub-compressed files to extract and check
    if current_depth < max_depth {
        // Report progress: scanning/extracting phase (high-latency operation)
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "archive".to_string());
        reporter
            .update(
                InstallPhaseId::ResolvingPack,
                None,
                InstallPhaseDetails::Modpack {
                    project_id: None,
                    version_id: None,
                    title: Some(format!("Scanning {filename} (level {current_depth}/{max_depth})")),
                },
            )
            .await?;

        let state = State::get().await?;
        let scratch =
            crate::api::pack::archive_util::create_import_scratch_dir(&state)
                .await?;
        // Everything below this point owns `scratch`. A guard removes it on any
        // early return — including a failed progress emit — so no path can leak
        // an extracted copy of the archive into the cache directory.
        let mut scratch_guard = ScratchDirGuard::new(scratch.clone());

        // Extract the entire archive to check for sub-packs.
        //
        // Opening a zip, walking its central directory and inflating every
        // member is synchronous: the `zip` crate exposes no async reader. Doing
        // that inline parks the executor thread for as long as the archive
        // takes to decompress, so a large dropped pack froze every other task
        // on the runtime and the reporter could not even publish the next
        // phase. Run the whole scan on the blocking pool and bound it.
        //
        // Tokio cannot cancel a blocking task once it started, so the bound is
        // cooperative: raising this flag makes the worker stop at its next chunk
        // or entry boundary and delete `scratch` itself, after its own file
        // handles are closed (Windows refuses to unlink open files). Three
        // things can raise it: the timeout below, an error inside the worker,
        // and `ScanCancellationGuard` when this whole operation is dropped —
        // that last one is what covers a user cancel or a job unwind, where
        // nothing else would ever tell the worker to stop. The worker may finish
        // reading at most one in-flight chunk before it notices, which is the
        // residual we cannot remove.
        let scan_path = path.clone();
        let scan_scratch = scratch.clone();
        let cancel =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut cancel_guard =
            ScanCancellationGuard::new(std::sync::Arc::clone(&cancel));
        // Removal belongs to the worker for as long as it may hold handles, but
        // the guard keeps the flag so an abandoned operation can still cancel it
        // and retry the delete.
        scratch_guard.defer_to_worker(std::sync::Arc::clone(&cancel));
        let scan_cancel = std::sync::Arc::clone(&cancel);
        let scan = tokio::time::timeout(
            PACK_ARCHIVE_SCAN_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                scan_sub_archives(&scan_path, &scan_scratch, &scan_cancel)
            }),
        )
        .await;
        let sub_archive_paths = match scan {
            Ok(Ok(Ok(paths))) => {
                // The worker has been joined, so its handles are closed and the
                // flag no longer has anything to stop.
                cancel_guard.disarm();
                scratch_guard.reclaim_after_worker();
                paths
            }
            Ok(Ok(Err(error))) => {
                // The worker already removed `scratch` on its own error path.
                scratch_guard.disarm_missing();
                return Err(error);
            }
            Ok(Err(join_error)) => {
                // A panic in the worker means its cleanup may not have run.
                scratch_guard.reclaim_after_worker();
                return Err(crate::ErrorKind::OtherError(format!(
                    "Modpack archive scan failed: {join_error}"
                ))
                .into());
            }
            Err(_) => {
                // Raise the flag now and stand down the RAII guard: the worker
                // still owns `scratch`, so it removes the directory after its
                // handles are closed rather than racing this scope's guard.
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                cancel_guard.disarm();
                return Err(crate::ErrorKind::InputError(format!(
                    "Timed out while reading {filename} as a modpack archive"
                ))
                .into());
            }
        };
        if !sub_archive_paths.is_empty() {
            reporter
                .update(
                    InstallPhaseId::ExtractingOverrides,
                    Some(InstallProgress {
                        current: sub_archive_paths.len() as u64,
                        total: sub_archive_paths.len() as u64,
                        secondary: None,
                    }),
                    InstallPhaseDetails::Modpack {
                        project_id: None,
                        version_id: None,
                        title: Some(format!("Extracting {filename}")),
                    },
                )
                .await?;
        }

        // Try to install each sub-archive recursively
        for sub_path in sub_archive_paths {
            let result = install_local_pack_file_recursive(
                sub_path,
                instance_id.clone(),
                reporter.clone(),
                current_depth + 1,
                max_depth,
                capture_game_options,
            )
            .await;

            if result.is_ok() {
                return result;
            }
        }

        drop(scratch_guard);
    }

    // If all else fails, return error
    Err(ErrorKind::InputError(
        "Unrecognized modpack format: no known pack manifest was found in the archive"
            .to_string(),
    ).into())
}

/// Dispatches a local non-mrpack modpack file to its format-specific
/// installer, based on the detected pack format.
#[async_recursion::async_recursion]
async fn install_local_pack_file(
    detected: crate::api::pack::detect::DetectedLocalPack,
    path: PathBuf,
    instance_id: String,
    reporter: InstallProgressReporter,
    capture_game_options: bool,
) -> crate::Result<InstallExecutionOutcome<()>> {
    use crate::api::pack::detect::LocalPackFormat;

    let source_filename = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string());
    match detected.format {
        LocalPackFormat::Mrpack => {
            let create_pack =
                generate_pack_from_file(path, instance_id.clone()).await?;
            return Ok(
                match install_zipped_mrpack_files_with_reporter(
                    create_pack,
                    false,
                    DownloadReason::Modpack,
                    reporter,
                    capture_game_options,
                )
                .await?
                {
                    MrpackInstallOutcome::Completed => {
                        InstallExecutionOutcome::Completed(())
                    }
                    MrpackInstallOutcome::WaitingForUser(reason) => {
                        InstallExecutionOutcome::WaitingForUser(reason)
                    }
                },
            );
        }
        LocalPackFormat::CurseForge => {
            let skipped_missing_content_paths = reporter
                .current_state()
                .await?
                .skipped_missing_content_paths;
            let result = crate::api::curseforge::install_modpack_from_local_archive_with_reporter(
                instance_id.clone(),
                path,
                detected.base_folder,
                source_filename,
                false,
                reporter,
                crate::launcher::InstanceCompletionPolicy::DeferToInstallJob,
            )
            .await?;
            if let Some(reason) = curseforge_manual_download_pause(
                &result,
                &skipped_missing_content_paths,
            ) {
                return Ok(InstallExecutionOutcome::WaitingForUser(reason));
            }
            if capture_game_options {
                crate::api::instance::capture_pack_game_options_base(
                    instance_id.clone(),
                    result.game_options_source,
                    result.pack_servers,
                )
                .await?;
            } else {
                crate::api::instance::capture_pack_server_base(
                    instance_id.clone(),
                    result.pack_servers,
                )
                .await?;
            }
        }
        LocalPackFormat::Mcbbs => {
            crate::api::pack::install_mcbbs::install_mcbbs_pack_with_reporter(
                instance_id,
                path,
                detected.base_folder,
                source_filename,
                reporter,
            )
            .await?;
        }
        LocalPackFormat::Hmcl => {
            crate::api::pack::install_hmcl::install_hmcl_pack_with_reporter(
                instance_id,
                path,
                detected.base_folder,
                source_filename,
                reporter,
            )
            .await?;
        }
        LocalPackFormat::MmcExport => {
            crate::api::pack::install_mmc_zip::install_mmc_zip_with_reporter(
                instance_id,
                path,
                detected.base_folder,
                source_filename,
                reporter,
            )
            .await?;
        }
        LocalPackFormat::LauncherBundled => {
            let inner_entry = detected.inner_pack_entry.ok_or_else(|| {
                ErrorKind::InputError(
                    "Launcher bundle is missing its inner modpack file"
                        .to_string(),
                )
            })?;
            let state = State::get().await?;
            let scratch =
                crate::api::pack::archive_util::create_import_scratch_dir(
                    &state,
                )
                .await?;
            let inner_name = inner_entry
                .rsplit('/')
                .next()
                .unwrap_or("modpack.zip")
                .to_string();
            let inner_path = scratch.join(&inner_name);
            crate::api::pack::archive_util::extract_archive_entry_to_file(
                path,
                inner_entry,
                inner_path.clone(),
            )
            .await?;

            // Use our recursive function to install the inner pack
            let result = install_local_pack_file_recursive(
                inner_path,
                instance_id,
                reporter,
                1, // already one level deep
                3,
                capture_game_options,
            )
            .await;

            // Clean up temporary directory
            if let Err(error) = tokio::fs::remove_dir_all(&scratch).await {
                tracing::warn!(
                    "Failed to clean up modpack import scratch directory {}: {error}",
                    scratch.display()
                );
            }

            return result;
        }
        LocalPackFormat::PlainArchive => {
            let version_id = detected.plain_version_id.ok_or_else(|| {
                ErrorKind::InputError(
                    "Could not locate the instance version in the archive"
                        .to_string(),
                )
            })?;
            crate::api::pack::install_plain_archive::install_plain_archive_with_reporter(
                instance_id,
                path,
                detected.base_folder,
                version_id,
                source_filename,
                reporter,
            )
            .await?;
        }
        LocalPackFormat::InstanceFolder => {
            // Extract the base folder contents to a temporary directory
            let state = State::get().await?;
            let scratch =
                crate::api::pack::archive_util::create_import_scratch_dir(
                    &state,
                )
                .await?;

            // Extract the instance folder contents
            crate::api::pack::archive_util::extract_archive_subdir(
                path,
                detected.base_folder,
                scratch.clone(),
            )
            .await?;

            // Now import it as a generic instance
            let details = InstallPhaseDetails::Modpack {
                project_id: None,
                version_id: None,
                title: source_filename.clone(),
            };

            crate::api::pack::import::generic::import_generic(
                scratch,
                instance_id,
                reporter,
                details,
                false,
                &crate::api::pack::import::ImportOverrides::default(),
                None, // Not compatible mode
            )
            .await?;
        }
    }
    Ok(InstallExecutionOutcome::Completed(()))
}

pub(super) fn curseforge_manual_download_pause(
    result: &crate::api::curseforge::CurseForgeModpackInstallResult,
    skipped_missing_content_paths: &[String],
) -> Option<InstallPauseReason> {
    let missing_downloads = result
        .content
        .manual_downloads
        .iter()
        .filter(|download| {
            !skipped_missing_content_paths.contains(&download.file_name)
        })
        .collect::<Vec<_>>();
    if missing_downloads.is_empty() {
        return None;
    }
    Some(InstallPauseReason::MissingRequiredContent {
        failed_files: missing_downloads.len() as u64,
        paths: missing_downloads
            .iter()
            .map(|download| download.file_name.clone())
            .collect(),
    })
}

pub(super) fn curseforge_world_was_imported_manually(
    job_state: &InstallJobState,
    request: &crate::api::curseforge::CurseForgeWorldInstallRequest,
) -> bool {
    let project_id = request.project_id.to_string();
    let file_id = request.file_id.to_string();
    job_state.download_items().iter().any(|item| {
        item.status == crate::install::model::DownloadItemStatus::Completed
            && item.project_id.as_deref() == Some(project_id.as_str())
            && item.version_id.as_deref() == Some(file_id.as_str())
    })
}

#[cfg(test)]
mod archive_scan_tests {
    use super::*;

    /// Builds a minimal stored-only zip in memory.
    ///
    /// Hand-written rather than produced by the `zip` writer so the fixture
    /// exercises exactly the reader the installer uses, without depending on
    /// the writer API of the pinned crate version.
    /// A cancellation flag that is never raised.
    fn no_cancel() -> std::sync::atomic::AtomicBool {
        std::sync::atomic::AtomicBool::new(false)
    }

    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let declared = entries
            .iter()
            .map(|(name, body)| (*name, *body, body.len() as u32))
            .collect::<Vec<_>>();
        build_archive_with_declared_sizes(&declared)
    }

    /// Builds an archive whose uncompressed size fields can lie.
    ///
    /// A crafted or truncated pack can declare a member larger than the bytes
    /// it actually stores; the scan has to notice instead of writing a short
    /// file that the recursive pass would then parse as a nested pack.
    ///
    /// The two size fields mean different things and must not be conflated:
    /// the *compressed* size is the number of bytes physically stored in the
    /// archive (here `body.len()`), while the *uncompressed* size is the
    /// intentionally false declaration the test is exercising. Writing the
    /// false value into the compressed field too would make the reader's
    /// `Take` bound swallow following archive bytes and never produce a short
    /// read, so the scan would see a corrupt-stream error instead of the
    /// truncation it is meant to detect.
    fn build_archive_with_declared_sizes(
        entries: &[(&str, &[u8], u32)],
    ) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffff_u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    let mask = 0_u32.wrapping_sub(crc & 1);
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        let mut out = Vec::new();
        let mut directory = Vec::new();
        let mut count = 0_u16;
        for (name, body, declared) in entries {
            let offset = out.len() as u32;
            let crc = crc32(body);
            let stored = body.len() as u32;
            let declared = *declared;
            out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
            out.extend_from_slice(&20_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            // Compressed size = bytes actually stored; uncompressed size = the
            // declaration under test. Both headers agree with each other, which
            // is what a real truncated member looks like.
            out.extend_from_slice(&stored.to_le_bytes());
            out.extend_from_slice(&declared.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(body);

            directory.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
            directory.extend_from_slice(&20_u16.to_le_bytes());
            directory.extend_from_slice(&20_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&crc.to_le_bytes());
            directory.extend_from_slice(&stored.to_le_bytes());
            directory.extend_from_slice(&declared.to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u16.to_le_bytes());
            directory.extend_from_slice(&0_u32.to_le_bytes());
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
            count += 1;
        }

        let directory_offset = out.len() as u32;
        let directory_size = directory.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&directory_size.to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out
    }

    #[test]
    fn scanning_extracts_nested_packs_and_ignores_everything_else() {
        // A dropped archive is not itself a pack, so detection fails and the
        // installer has to look inside. Only nested archives may be written
        // out; ordinary members must not leak into the scratch directory.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("dropped.zip");
        let inner_pack = build_archive(&[(
            "modrinth.index.json",
            br#"{"formatVersion":1,"game":"minecraft","versionId":"1","name":"tiny","files":[],"dependencies":{"minecraft":"1.20.1"}}"#,
        )]);
        std::fs::write(
            &outer,
            build_archive(&[
                ("readme.txt", b"not a pack"),
                ("pack/inner.mrpack", &inner_pack),
                ("assets/extra.zip", &build_archive(&[])),
            ]),
        )
        .unwrap();

        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let found = scan_sub_archives(&outer, &scratch, &no_cancel()).unwrap();

        let mut names = found
            .iter()
            .map(|path| {
                path.strip_prefix(&scratch)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["assets/extra.zip", "pack/inner.mrpack"]);
        let extracted = found
            .iter()
            .find(|path| path.to_string_lossy().ends_with("inner.mrpack"))
            .unwrap();
        assert_eq!(
            std::fs::read(extracted).unwrap(),
            inner_pack,
            "a nested pack must survive byte-for-byte so the next pass can read its manifest"
        );
        assert!(
            !scratch.join("readme.txt").exists(),
            "non-archive members must not be extracted"
        );
    }

    #[test]
    fn a_member_that_is_not_a_zip_reports_an_error_instead_of_hanging() {
        // The reported stall class: the parser must return, never spin. A file
        // that is not a zip at all has to produce the "unrecognized format"
        // error the phase maps to `unrecognized_format`.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("truncated.zip");
        std::fs::write(&outer, b"PK\x03\x04 truncated central directory")
            .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let error =
            scan_sub_archives(&outer, &scratch, &no_cancel()).unwrap_err();

        assert!(
            error.to_string().contains("Unrecognized modpack format"),
            "expected the unrecognized-format error, got: {error}"
        );
    }

    #[test]
    fn an_archive_without_nested_packs_finishes_empty() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("plain.zip");
        std::fs::write(
            &outer,
            build_archive(&[("only.txt", b"nothing to unpack")]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        assert!(
            scan_sub_archives(&outer, &scratch, &no_cancel())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_raised_cancel_flag_stops_the_scan_before_it_writes() {
        // Tokio cannot cancel a running blocking task, so this flag is the only
        // thing that ends the scan after the caller's timeout fires. A scan
        // that ignored it would keep inflating into a directory the installer
        // is already deleting.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("dropped.zip");
        std::fs::write(
            &outer,
            build_archive(&[("pack/inner.mrpack", b"payload")]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(true);

        let error = scan_sub_archives(&outer, &scratch, &cancel).unwrap_err();

        assert!(
            error.to_string().contains("canceled"),
            "expected the cancellation error, got: {error}"
        );
        assert!(
            !scratch.exists(),
            "a canceled scan must clean its own scratch directory"
        );
    }

    #[test]
    fn a_failed_scan_removes_its_own_scratch() {
        // Malformed input is the plain error path: the worker must not leave an
        // extracted copy behind when it bails out before finishing.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("bad.zip");
        std::fs::write(&outer, b"not a zip at all").unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("partial.bin"), b"leftover").unwrap();

        scan_sub_archives(&outer, &scratch, &no_cancel()).unwrap_err();

        assert!(
            !scratch.exists(),
            "a failed scan must not leave an extracted copy behind"
        );
    }

    #[test]
    fn a_scan_canceled_midway_stops_before_the_next_entry_and_cleans_up() {
        // The real contract: a flag raised *while the worker is running* has to
        // end the scan. A pre-raised flag would only prove the loop guard works
        // on entry zero; this proves an in-progress scan observes it, stops
        // before writing the next member, and removes its own scratch.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("dropped.zip");
        std::fs::write(
            &outer,
            build_archive(&[
                ("pack/first.mrpack", b"first payload"),
                ("pack/second.mrpack", b"second payload"),
                ("pack/third.mrpack", b"third payload"),
            ]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let cancel =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&cancel);
        AFTER_ENTRY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }));
        });
        let result = scan_sub_archives(&outer, &scratch, &cancel);
        AFTER_ENTRY.with(|hook| *hook.borrow_mut() = None);

        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("canceled"),
            "a cancellation must surface as the cancellation error, got: {error}"
        );
        assert!(
            !scratch.exists(),
            "a scan canceled mid-flight must remove its own scratch"
        );
        assert!(
            cancel.load(std::sync::atomic::Ordering::Relaxed),
            "the test hook must actually have raised the flag mid-scan"
        );
    }

    #[test]
    fn a_successful_scan_leaves_scratch_for_the_caller_to_remove() {
        // The guard in the installer removes it; the scan must not, or the
        // recursive pass would lose the files it is about to read.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("dropped.zip");
        std::fs::write(
            &outer,
            build_archive(&[("pack/inner.mrpack", b"payload")]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let found = scan_sub_archives(&outer, &scratch, &no_cancel()).unwrap();

        assert_eq!(found.len(), 1);
        assert!(scratch.exists(), "successful scans keep scratch");
        assert_eq!(std::fs::read(&found[0]).unwrap(), b"payload");
    }

    #[test]
    fn a_member_shorter_than_its_declared_size_is_rejected() {
        // Truncation: the entry promises 4096 bytes but the archive only holds
        // 14. Writing that short file and handing it to the recursive pass
        // would make the installer parse garbage as a nested pack.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("truncated-member.zip");
        std::fs::write(
            &outer,
            build_archive_with_declared_sizes(&[(
                "pack/inner.mrpack",
                b"only sixteen b",
                4096,
            )]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let error =
            scan_sub_archives(&outer, &scratch, &no_cancel()).unwrap_err();

        assert!(
            error.to_string().contains("declares 4096 bytes"),
            "expected the truncation error, got: {error}"
        );
        assert!(!scratch.exists(), "a rejected scan must clean up");
    }

    #[test]
    fn an_entry_over_the_supplied_budget_is_rejected_before_any_output() {
        // Same branch production hits with the shared 8 GiB limit, driven with a
        // 4-byte budget so no large fixture is needed. The declared size (64)
        // exceeds it, so the scan must reject before creating the output file:
        // if it extracted first, the member would exist on disk afterwards.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("over-budget.zip");
        std::fs::write(
            &outer,
            build_archive_with_declared_sizes(&[(
                "pack/inner.mrpack",
                b"payload",
                64,
            )]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let error = scan_sub_archives_within(&outer, &scratch, &no_cancel(), 4)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("exceeded its 4-byte extraction budget"),
            "expected the budget error, got: {error}"
        );
        assert!(!scratch.exists(), "a rejected scan must clean up");
    }

    #[test]
    fn a_scan_inside_the_supplied_budget_still_extracts() {
        // Counterpart to the test above: the same fixture with a budget that
        // fits must succeed, so the rejection is caused by the budget and not
        // by the fixture shape.
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("within-budget.zip");
        std::fs::write(
            &outer,
            build_archive_with_declared_sizes(&[(
                "pack/inner.mrpack",
                b"payload",
                7,
            )]),
        )
        .unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();

        let found =
            scan_sub_archives_within(&outer, &scratch, &no_cancel(), 64)
                .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(std::fs::read(&found[0]).unwrap(), b"payload");
    }

    #[test]
    fn dropping_the_cancellation_guard_raises_the_shared_flag() {
        // The RAII fallback for an abandoned operation: a user cancel or a job
        // unwind drops this guard without going through the timeout arm, and the
        // worker only learns about it through this flag.
        let flag =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let _guard =
                ScanCancellationGuard::new(std::sync::Arc::clone(&flag));
            assert!(
                !flag.load(std::sync::atomic::Ordering::Relaxed),
                "constructing the guard must not cancel anything"
            );
        }
        assert!(
            flag.load(std::sync::atomic::Ordering::Relaxed),
            "dropping the guard must raise the flag"
        );
    }

    #[test]
    fn a_disarmed_cancellation_guard_leaves_the_flag_alone() {
        // The success path disarms before dropping, so a clean scan must not
        // look cancelled to the worker's final check.
        let flag =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let mut guard =
                ScanCancellationGuard::new(std::sync::Arc::clone(&flag));
            guard.disarm();
        }
        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn a_deferred_scratch_guard_signals_cancellation_and_removes_an_idle_dir() {
        // The external-cancel handoff: while deferred the worker owns removal,
        // but the abandoned scope must still raise the flag and clean up when
        // nothing holds the directory. This is not the spawn_blocking race, only
        // the RAII fallback that race relies on.
        let temp = tempfile::tempdir().unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("stale.bin"), b"orphan").unwrap();
        let flag =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        {
            let mut guard = ScratchDirGuard::new(scratch.clone());
            guard.defer_to_worker(std::sync::Arc::clone(&flag));
            assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
        }

        assert!(
            flag.load(std::sync::atomic::Ordering::Relaxed),
            "a deferred drop must tell the worker to stop"
        );
        assert!(
            !scratch.exists(),
            "a deferred drop must remove a directory nothing is using"
        );
    }

    #[test]
    fn a_reclaimed_scratch_guard_deletes_without_cancelling() {
        // The success path reclaims before dropping: it deletes the scratch the
        // recursive pass no longer needs, and must not look like a cancel.
        let temp = tempfile::tempdir().unwrap();
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let flag =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        {
            let mut guard = ScratchDirGuard::new(scratch.clone());
            guard.defer_to_worker(std::sync::Arc::clone(&flag));
            guard.reclaim_after_worker();
        }

        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!scratch.exists(), "a reclaimed guard still cleans up");
    }
}
