import assert from 'node:assert/strict'
import test from 'node:test'

import { createStartupJavaScan } from './startup-java-scan.ts'

test('starts one Java scan and handles scan failures without throwing', async () => {
    const failure = new Error('scan failed')
    let calls = 0
    let reportedError: unknown
    const startScan = createStartupJavaScan(
        async () => {
            calls += 1
            throw failure
        },
        (error) => {
            reportedError = error
        },
    )

    assert.doesNotThrow(() => {
        startScan()
        startScan()
    })
    await Promise.resolve()

    assert.equal(calls, 1)
    assert.equal(reportedError, failure)
})
