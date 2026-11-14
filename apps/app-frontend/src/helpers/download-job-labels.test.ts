import assert from 'node:assert/strict'
import test from 'node:test'

import type { InstallJobSnapshot } from '@/helpers/install'

import { downloadFallbackLabelKind, downloadJobPhaseLabel } from './download-job-labels.ts'

function job(overrides: Partial<InstallJobSnapshot> = {}): InstallJobSnapshot {
    return {
        job_id: 'job-a',
        status: 'running',
        execution_mode: 'normal',
        kind: 'install_pack',
        phase: 'finalizing',
        items: [],
        summary: { files_completed: 0, bytes_downloaded: 0, fallback_count: 0 },
        ...overrides,
    } as InstallJobSnapshot
}

test('a succeeded job reads completed even when its stored phase is still finalizing', () => {
    assert.deepEqual(downloadJobPhaseLabel(job({ status: 'succeeded', phase: 'finalizing' })), {
        kind: 'status',
        status: 'succeeded',
    })
})

test('failed and canceled jobs report their status instead of the rollback phase', () => {
    assert.deepEqual(downloadJobPhaseLabel(job({ status: 'failed', phase: 'rolling_back' })), {
        kind: 'status',
        status: 'failed',
    })
    assert.deepEqual(downloadJobPhaseLabel(job({ status: 'canceled', phase: 'rolling_back' })), {
        kind: 'status',
        status: 'canceled',
    })
    assert.deepEqual(downloadJobPhaseLabel(job({ status: 'interrupted', phase: 'rolling_back' })), {
        kind: 'status',
        status: 'interrupted',
    })
})

test('non-terminal jobs still describe the phase they are working in', () => {
    assert.deepEqual(downloadJobPhaseLabel(job({ phase: 'applying_content' })), {
        kind: 'phase',
        phase: 'applying_content',
    })
    assert.deepEqual(
        downloadJobPhaseLabel(job({ kind: 'install_content', phase: 'preparing_instance' })),
        { kind: 'legacyContentPhase' },
    )
})

test('a still-running job with a rollback failure keeps the cleanup warning', () => {
    assert.deepEqual(
        downloadJobPhaseLabel(
            job({ status: 'running', rollback_error: { code: 'x', message: 'y' } as never }),
        ),
        { kind: 'cleanupIncomplete' },
    )
})

test('fallback counts are historical once the job is terminal and absent during recovery validation', () => {
    const summary = { files_completed: 0, bytes_downloaded: 0, fallback_count: 68 }
    assert.equal(downloadFallbackLabelKind(job({ status: 'running', summary })), 'active')
    assert.equal(downloadFallbackLabelKind(job({ status: 'succeeded', summary })), 'historical')
    assert.equal(downloadFallbackLabelKind(job({ status: 'failed', summary })), 'historical')
    assert.equal(downloadFallbackLabelKind(job({ status: 'canceled', summary })), 'historical')
    assert.equal(
        downloadFallbackLabelKind(
            job({ status: 'running', execution_mode: 'recovery_validation', summary }),
        ),
        null,
    )
    assert.equal(downloadFallbackLabelKind(job()), null)
})
