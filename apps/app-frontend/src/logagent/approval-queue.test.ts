import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createApprovalQueue } from './approval-queue.ts'

const tool = { name: 'create_instance', description: 'Create an instance' }

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
