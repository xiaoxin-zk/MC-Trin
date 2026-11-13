/** Stall detection for `wait_for_install_job`.
 *
 * Kept apart from `install.ts` so it can be unit-tested without pulling in the
 * Tauri event bindings that module imports at load time. Nothing here touches
 * the runtime: the helpers are pure functions over an observable snapshot.
 */

/** The subset of an install-job snapshot that proves a job moved. */
export interface InstallProgressMarker {
    phase: string
    current: number | null
    secondary: number | null
    bytes: number
}

/** The fields the marker reads. Structurally typed so callers need no import. */
export interface InstallProgressSnapshot {
    phase: string
    progress?: { current: number; secondary?: { current: number } | null } | null
    summary: { bytes_downloaded: number }
}

/**
 * The tuple that says whether an install job actually moved.
 *
 * A successful `install_job_get` only proves the backend answered; a wedged job
 * answers with the same numbers every second. Tracking the observable progress
 * instead of "the query returned" is what makes the stall timer mean what its
 * name says.
 *
 * `bytes_downloaded` is included because content downloads report byte-level
 * progress without changing `progress.current` (which counts files): a single
 * multi-gigabyte file can advance for many minutes inside one file slot.
 */
export function installProgressMarker(job: InstallProgressSnapshot): InstallProgressMarker {
    return {
        phase: job.phase,
        current: job.progress?.current ?? null,
        secondary: job.progress?.secondary?.current ?? null,
        bytes: job.summary.bytes_downloaded,
    }
}

/** Whether `next` shows any movement over `previous`. */
export function progressMarkerAdvanced(
    previous: InstallProgressMarker | null,
    next: InstallProgressMarker,
): boolean {
    if (!previous) return true
    return (
        previous.phase !== next.phase ||
        previous.current !== next.current ||
        previous.secondary !== next.secondary ||
        previous.bytes !== next.bytes
    )
}
