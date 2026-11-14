import assert from 'node:assert/strict'
import { mock, test } from 'node:test'

import { createApprovalQueue } from './approval-queue.ts'

const tool = { name: 'create_instance', description: 'Create an instance' }

/** Runs `body` with the fake clock; timers never fire unless the test ticks them. */
async function withFakeTimers(body: () => Promise<void>): Promise<void> {
    mock.timers.enable({ apis: ['setTimeout'] })
    try {
        await body()
    } finally {
        mock.timers.reset()
    }
}

test('a request is answered by resolve and leaves the queue', async () => {
    const queue = createApprovalQueue(120_000, () => assert.fail('must not time out'))
    const answer = queue.request('MC-Trin', tool, { instance_name: 'Survival' })
    assert.equal(queue.current.value?.tool.name, 'create_instance')
    queue.resolve(true)
    assert.equal(await answer, true)
    assert.equal(queue.current.value, null)
})

test('a denied request resolves false and shows nothing more', async () => {
    const queue = createApprovalQueue(120_000, () => assert.fail('must not time out'))
    const answer = queue.request('MC-Trin', tool, {})
    queue.resolve(false)
    assert.equal(await answer, false)
    assert.deepEqual(queue.pending.value, [])
})

test('denyAll settles every queued request', async () => {
    const queue = createApprovalQueue(120_000, () => assert.fail('must not time out'))
    const first = queue.request('MC-Trin', tool, { instance_name: 'A' })
    const second = queue.request('MC-Trin', tool, { instance_name: 'B' })
    queue.denyAll()
    assert.deepEqual(await Promise.all([first, second]), [false, false])
    assert.deepEqual(queue.pending.value, [])
})

test('an unanswered request times out as denied and reports the tool', async () => {
    const timedOut: string[] = []
    const queue = createApprovalQueue(5, (timedOutTool) => timedOut.push(timedOutTool.name))
    const answer = queue.request('MC-Trin', tool, {})
    // The dialog must disappear on its own, or the user keeps seeing a request
    // whose promise no longer exists.
    await new Promise((resolve) => setTimeout(resolve, 20))
    assert.equal(await answer, false)
    assert.deepEqual(timedOut, ['create_instance'])
    assert.deepEqual(queue.pending.value, [])
})

test('answering before the deadline clears the timer', async () => {
    const timedOut: string[] = []
    const queue = createApprovalQueue(5, (timedOutTool) => timedOut.push(timedOutTool.name))
    const answer = queue.request('MC-Trin', tool, {})
    queue.resolve(true)
    assert.equal(await answer, true)
    await new Promise((resolve) => setTimeout(resolve, 20))
    assert.deepEqual(timedOut, [], 'a settled approval must not report a timeout')
})

test('the timeout does not resolve a dialog twice', async () => {
    let notifications = 0
    const queue = createApprovalQueue(5, () => notifications++)
    const answer = queue.request('MC-Trin', tool, {})
    await new Promise((resolve) => setTimeout(resolve, 15))
    queue.resolve(true)
    assert.equal(await answer, false)
    assert.equal(notifications, 1)
})

test('a queued request gets its full visible budget after the head is answered late', async () => {
    await withFakeTimers(async () => {
        const timedOut: string[] = []
        const queue = createApprovalQueue(120_000, (timedOutTool) =>
            timedOut.push(timedOutTool.name),
        )
        const first = queue.request('MC-Trin', tool, { instance_name: 'A' })
        const second = queue.request('MC-Trin', tool, { instance_name: 'B' })
        assert.equal(queue.current.value?.args.instance_name, 'A', 'only the head is displayed')

        // The user reads the first dialog for 90 s. Nothing may expire behind it.
        mock.timers.tick(90_000)
        assert.deepEqual(timedOut, [])
        queue.resolve(true)
        assert.equal(await first, true)
        assert.equal(queue.current.value?.args.instance_name, 'B')

        // The second dialog is only now on screen; it must get the whole 120 s,
        // not the 30 s the old request-time deadline would have left it.
        mock.timers.tick(119_999)
        assert.deepEqual(timedOut, [], 'the queued request must not expire while still visible')
        assert.equal(queue.current.value?.args.instance_name, 'B')

        mock.timers.tick(1)
        assert.deepEqual(timedOut, ['create_instance'], 'exactly one notification for one expiry')
        assert.equal(await second, false)
        assert.equal(queue.current.value, null)
    })
})

test('a queued request starts its budget when the head expires, not when it was requested', async () => {
    await withFakeTimers(async () => {
        const timedOut: string[] = []
        const queue = createApprovalQueue(120_000, (timedOutTool) =>
            timedOut.push(timedOutTool.name),
        )
        const first = queue.request('MC-Trin', tool, { instance_name: 'A' })
        const second = queue.request('MC-Trin', tool, { instance_name: 'B' })

        mock.timers.tick(120_000)
        assert.deepEqual(timedOut, ['create_instance'], 'only the visible dialog expires')
        assert.equal(await first, false)
        assert.equal(queue.current.value?.args.instance_name, 'B')

        mock.timers.tick(119_999)
        assert.deepEqual(timedOut, ['create_instance'])
        mock.timers.tick(1)
        assert.deepEqual(timedOut, ['create_instance', 'create_instance'])
        assert.equal(await second, false)
        assert.deepEqual(queue.pending.value, [])
    })
})
