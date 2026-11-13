import assert from 'node:assert/strict'
import test from 'node:test'

import { installProgressMarker, progressMarkerAdvanced } from './install-stall.ts'

/** Only the fields the stall marker reads. */
interface Snapshot {
    phase: string
    progress?: { current: number; secondary?: { current: number } | null } | null
    summary: { bytes_downloaded: number }
}

function job(overrides: {
    phase?: string
    current?: number | null
    secondary?: number | null
    bytes?: number
}): Snapshot {
    const { phase = 'downloading_content', current = 0, secondary = null, bytes = 0 } = overrides
    return {
        phase,
        progress:
            current === null
                ? null
                : {
                      current,
                      secondary: secondary === null ? null : { current: secondary },
                  },
        summary: { bytes_downloaded: bytes },
    }
}

test('the first snapshot always counts as movement', () => {
    // Establishes the baseline: without this the very first poll would not arm
    // the timer and a job that never progressed would never time out.
    assert.equal(progressMarkerAdvanced(null, installProgressMarker(job({}))), true)
})

test('an identical repeated snapshot is not movement', () => {
    // The regression this guards: a wedged backend keeps answering every second
    // with the same numbers, and the old code re-armed the timer on each answer.
    const previous = installProgressMarker(job({ current: 3, bytes: 1024 }))
    const repeated = installProgressMarker(job({ current: 3, bytes: 1024 }))

    assert.equal(progressMarkerAdvanced(previous, repeated), false)
})

test('a file-count change is movement', () => {
    const previous = installProgressMarker(job({ current: 3 }))
    assert.equal(progressMarkerAdvanced(previous, installProgressMarker(job({ current: 4 }))), true)
})

test('a byte change inside one file is movement', () => {
    // Content downloads advance `summary.bytes_downloaded` without changing
    // `progress.current`, which counts files. A huge single file must not look
    // stalled for its whole transfer.
    const previous = installProgressMarker(job({ current: 3, bytes: 1024 }))
    const advanced = installProgressMarker(job({ current: 3, bytes: 2048 }))
    assert.equal(progressMarkerAdvanced(previous, advanced), true)
})

test('a phase change is movement', () => {
    const previous = installProgressMarker(job({ phase: 'resolving_pack' }))
    const next = installProgressMarker(job({ phase: 'downloading_content' }))
    assert.equal(progressMarkerAdvanced(previous, next), true)
})

test('a secondary-progress change is movement', () => {
    const previous = installProgressMarker(job({ current: 1, secondary: 10 }))
    const next = installProgressMarker(job({ current: 1, secondary: 20 }))
    assert.equal(progressMarkerAdvanced(previous, next), true)
})

test('a missing progress object is distinguishable from zero progress', () => {
    // `progress: null` means "no measurable progress yet", not "0 of N". Both
    // must still be tracked so a phase that later gains a counter reads as
    // movement rather than as a stall that already ran out the clock.
    const none = installProgressMarker(job({ current: null }))
    const zero = installProgressMarker(job({ current: 0 }))

    assert.equal(none.current, null)
    assert.equal(zero.current, 0)
    assert.equal(progressMarkerAdvanced(none, zero), true)
})
