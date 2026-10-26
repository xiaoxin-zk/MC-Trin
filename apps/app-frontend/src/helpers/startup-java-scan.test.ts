import assert from 'node:assert/strict'
import test from 'node:test'

import { createStartupJavaScan } from './startup-java-scan.ts'

test('starts one Java scan and handles scan failures without throwing', async () => {
    const failure = new Error('scan failed')
    let calls = 0
    const reportedError = new Promise<unknown>((resolve) => {
        const startScan = createStartupJavaScan(async () => {
            calls += 1
            throw failure
        }, resolve)

        assert.doesNotThrow(() => {
            startScan()
            startScan()
        })
    })

    assert.equal(await reportedError, failure)
    assert.equal(calls, 1)
})
