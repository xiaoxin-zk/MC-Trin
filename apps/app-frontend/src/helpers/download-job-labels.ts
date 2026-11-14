import type { InstallJobSnapshot, InstallPhaseId } from './install'
import { isInstallJobFinished } from './install'

/**
 * What the Downloads row subtitle should describe.
 *
 * A terminal status is authoritative: `phase` keeps the step the worker was last in (a
 * succeeded job stays at `finalizing` when the finalize step did not report `completed`,
 * and a cancelled job stays at `rolling_back`). Rendering that phase next to a green
 * "Completed" or red "Canceled" badge is what made a finished download contradict itself.
 */
export type DownloadJobPhaseLabel =
    | { kind: 'status'; status: 'succeeded' | 'failed' | 'canceled' | 'interrupted' }
    | { kind: 'cleanupIncomplete' }
    | { kind: 'legacyContentPhase' }
    | { kind: 'verifyingFiles' }
    | { kind: 'phase'; phase: InstallPhaseId }

export function downloadJobPhaseLabel(job: InstallJobSnapshot): DownloadJobPhaseLabel {
    if (job.status === 'succeeded') return { kind: 'status', status: 'succeeded' }
    if (job.status === 'failed') return { kind: 'status', status: 'failed' }
    if (job.status === 'canceled') return { kind: 'status', status: 'canceled' }
    if (job.status === 'interrupted') return { kind: 'status', status: 'interrupted' }
    if (job.rollback_error) return { kind: 'cleanupIncomplete' }
    // Older queued content jobs were initialized with the generic instance phase.
    // Keep their label meaningful while they are resumed or waiting for the worker.
    if (job.kind === 'install_content' && job.phase === 'preparing_instance')
        return { kind: 'legacyContentPhase' }
    if (job.execution_mode === 'recovery_validation' && activeRequestCount(job) === 0)
        return { kind: 'verifyingFiles' }
    return { kind: 'phase', phase: job.phase }
}

/**
 * Whether the fallback count describes a live retry loop (non-terminal) or a historical
 * total. Finished jobs must not claim they are still falling back.
 */
export function downloadFallbackLabelKind(job: InstallJobSnapshot): 'active' | 'historical' | null {
    if (!job.summary.fallback_count) return null
    if (job.execution_mode === 'recovery_validation') return null
    return isInstallJobFinished(job.status) ? 'historical' : 'active'
}

function activeRequestCount(job: InstallJobSnapshot) {
    return job.items.filter((item) => item.status === 'downloading' && item.request_url).length
}
