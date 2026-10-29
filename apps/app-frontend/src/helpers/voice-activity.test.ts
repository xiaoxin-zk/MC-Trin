import { describe, expect, it } from 'vitest'

import { byteRms, microphoneLevelPercent } from './voice-activity.ts'

describe('byteRms', () => {
    it('is zero for digital silence', () => {
        expect(byteRms(new Uint8Array([128, 128, 128]))).toBe(0)
    })

    it('reaches full scale for a square wave between the byte extremes', () => {
        expect(byteRms(new Uint8Array([255, 1]))).toBeCloseTo(0.992, 2)
    })

    it('returns zero for an empty buffer', () => {
        expect(byteRms(new Uint8Array())).toBe(0)
    })
})

describe('microphoneLevelPercent', () => {
    it('maps full scale to 100 and the -60 dBFS floor to 0', () => {
        expect(microphoneLevelPercent(1)).toBe(100)
        expect(microphoneLevelPercent(0.001)).toBe(0)
    })

    it('sits near the middle for speech-level input', () => {
        expect(microphoneLevelPercent(0.06)).toBeGreaterThan(50)
        expect(microphoneLevelPercent(0.06)).toBeLessThan(70)
    })

    it('clamps non-finite and non-positive values to zero', () => {
        expect(microphoneLevelPercent(0)).toBe(0)
        expect(microphoneLevelPercent(Number.NaN)).toBe(0)
        expect(microphoneLevelPercent(-0.5)).toBe(0)
    })
})
