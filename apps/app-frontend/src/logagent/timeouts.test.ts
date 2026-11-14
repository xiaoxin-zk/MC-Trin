import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    APPROVAL_TIMEOUT_MS,
    TURN_DEADLINE_MS,
    TURN_DEADLINE_MESSAGE,
    TurnDeadlineError,
    withTurnDeadline,
} from './timeouts.ts'

test('approval wait mirrors the 120 s backend grant lifetime', () => {
    // APPROVAL_TTL in packages/app-lib/src/api/logs/logagent/mod.rs; a longer
    // wait would only surface a token the gate already refuses.
    assert.equal(APPROVAL_TIMEOUT_MS, 120_000)
})

test('the turn deadline is longer than the backend session budget', () => {
    // SESSION_TTL is 10 minutes in the same module, so the renderer must not
    // cut a run the launcher can still finish.
    assert.ok(TURN_DEADLINE_MS > 10 * 60_000)
})

test('an operation that finishes first resolves and clears its timer', async () => {
    let fired = false
    const value = await withTurnDeadline(
        Promise.resolve('done'),
        20,
        'too slow',
        () => (fired = true),
    )
    assert.equal(value, 'done')
    await new Promise((resolve) => setTimeout(resolve, 40))
    assert.equal(fired, false, 'the timer must be cleared once the turn settles')
})

test('a stalled operation rejects with the deadline message and aborts the run', async () => {
    let aborted = false
    await assert.rejects(
        withTurnDeadline(
            new Promise<string>(() => {}),
            10,
            TURN_DEADLINE_MESSAGE,
            () => (aborted = true),
        ),
        (error: unknown) =>
            error instanceof TurnDeadlineError && error.message === TURN_DEADLINE_MESSAGE,
    )
    assert.equal(aborted, true)
})

test('a failed operation keeps its own error instead of the deadline message', async () => {
    await assert.rejects(
        withTurnDeadline(
            Promise.reject(new Error('provider rejected the request')),
            50,
            'too slow',
        ),
        /provider rejected the request/,
    )
})
