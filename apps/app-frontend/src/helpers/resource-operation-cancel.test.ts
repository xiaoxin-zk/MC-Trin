import assert from 'node:assert/strict'
import test from 'node:test'

import { isCanceledResourceError, resourceOperationError } from './resource-operation-logic.ts'

const formatter = (descriptor: { id: string; defaultMessage?: string }) =>
    descriptor.id === 'app.resources.error.canceled'
        ? 'The content operation was cancelled.'
        : (descriptor.defaultMessage ?? descriptor.id)

test('both backend cancellation spellings are recognised', () => {
    // ResourceError::Canceled in packages/app-lib/src/resources/mod.rs
    assert.equal(isCanceledResourceError(new Error('resource request canceled')), true)
    // ErrorKind::InputError("resource operation canceled") in
    // packages/app-lib/src/resources/execution.rs, serialised with code "resourceError"
    assert.equal(
        isCanceledResourceError({
            code: 'resourceError',
            message: 'Invalid input: resource operation canceled',
        }),
        true,
    )
    assert.equal(
        isCanceledResourceError(new Error('Invalid input: resource operation canceled')),
        true,
    )
})

test('abort errors and unrelated failures are classified correctly', () => {
    assert.equal(isCanceledResourceError({ name: 'AbortError', message: 'aborted' }), true)
    assert.equal(
        isCanceledResourceError(new Error('resource request failed (HTTP status None)')),
        false,
    )
    assert.equal(
        isCanceledResourceError({ code: 'resourceError', message: 'HTTP status None' }),
        false,
    )
    assert.equal(isCanceledResourceError(new Error('A user-provided description')), false)
})

test('cancellation is localized instead of leaking the raw backend diagnostic', () => {
    assert.equal(
        resourceOperationError(new Error('resource operation canceled'), formatter),
        'The content operation was cancelled.',
    )
    assert.equal(
        resourceOperationError(new Error('resource request canceled'), formatter),
        'The content operation was cancelled.',
    )
})
