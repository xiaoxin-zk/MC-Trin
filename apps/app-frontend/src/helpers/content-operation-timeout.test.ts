import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    CONTENT_OPERATION_TIMEOUT_MS,
    withContentOperationTimeout,
} from './content-operation-timeout.ts'

test('resolves with the operation result before the budget elapses', async () => {
    const value = await withContentOperationTimeout(Promise.resolve('installed'), 'timed out', 50)
    assert.equal(value, 'installed')
})

test('rejects with the supplied message once the budget elapses', async () => {
    const pending = new Promise<string>(() => {})
    await assert.rejects(
        withContentOperationTimeout(pending, 'content operation timed out', 20),
        /content operation timed out/,
    )
})

test('propagates the operation failure unchanged', async () => {
    await assert.rejects(
        withContentOperationTimeout(
            Promise.reject(new Error('launcher is locked')),
            'timed out',
            50,
        ),
        /launcher is locked/,
    )
})

test('exposes a sane default budget for the page', () => {
    assert.equal(CONTENT_OPERATION_TIMEOUT_MS, 30_000)
})
