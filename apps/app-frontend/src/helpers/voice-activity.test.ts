import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { byteRms, microphoneLevelPercent } from './voice-activity.ts'

describe('byteRms', () => {
    it('is zero for digital silence', () => {
        assert.equal(byteRms(new Uint8Array([128, 128, 128])), 0)
    })

    it('reaches full scale for a square wave between the byte extremes', () => {
        assert.ok(Math.abs(byteRms(new Uint8Array([255, 1])) - 0.992) < 0.001)
    })

    it('returns zero for an empty buffer', () => {
        assert.equal(byteRms(new Uint8Array()), 0)
    })
})

describe('microphoneLevelPercent', () => {
    it('maps full scale to 100 and the -60 dBFS floor to 0', () => {
        assert.equal(microphoneLevelPercent(1), 100)
        assert.equal(microphoneLevelPercent(0.001), 0)
    })

    it('sits near the middle for speech-level input', () => {
        const percent = microphoneLevelPercent(0.06)
        assert.ok(percent > 50 && percent < 70)
    })

    it('clamps non-finite and non-positive values to zero', () => {
        assert.equal(microphoneLevelPercent(0), 0)
        assert.equal(microphoneLevelPercent(Number.NaN), 0)
        assert.equal(microphoneLevelPercent(-0.5), 0)
    })
})
