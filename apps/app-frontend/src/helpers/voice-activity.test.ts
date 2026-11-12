import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { byteRms, createVoiceActivityMonitor, microphoneLevelPercent } from './voice-activity.ts'

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

/** A stand-in for the Web Audio graph the monitor builds: each watcher gets
 * one analyser whose samples the test controls frame by frame. */
function installAudioEnvironment() {
    const frames: { id: number; callback: FrameRequestCallback }[] = []
    let nextFrameId = 1
    const analysers: { samples: Uint8Array }[] = []
    const original = {
        AudioContext: Reflect.get(globalThis, 'AudioContext'),
        requestAnimationFrame: Reflect.get(globalThis, 'requestAnimationFrame'),
        cancelAnimationFrame: Reflect.get(globalThis, 'cancelAnimationFrame'),
    }

    class FakeAudioContext {
        createMediaStreamSource() {
            return { connect() {}, disconnect() {} }
        }

        createAnalyser() {
            const entry = { samples: new Uint8Array(512).fill(128) }
            analysers.push(entry)
            return {
                fftSize: 512,
                smoothingTimeConstant: 0,
                connect() {},
                disconnect() {},
                getByteTimeDomainData(target: Uint8Array) {
                    target.set(entry.samples)
                },
            }
        }

        close() {
            return Promise.resolve()
        }
    }

    Object.defineProperty(globalThis, 'AudioContext', {
        configurable: true,
        value: FakeAudioContext,
    })
    Object.defineProperty(globalThis, 'requestAnimationFrame', {
        configurable: true,
        value: (callback: FrameRequestCallback) => {
            const id = nextFrameId
            nextFrameId += 1
            frames.push({ id, callback })
            return id
        },
    })
    Object.defineProperty(globalThis, 'cancelAnimationFrame', {
        configurable: true,
        value: (id: number) => {
            const index = frames.findIndex((frame) => frame.id === id)
            if (index >= 0) frames.splice(index, 1)
        },
    })

    return {
        analysers,
        tick(framesToRun = 1) {
            for (let index = 0; index < framesToRun; index += 1) {
                const frame = frames.shift()
                assert.ok(frame, 'the monitor should keep requesting frames')
                frame.callback(0)
            }
        },
        get pendingFrames() {
            return frames.length
        },
        restore() {
            Object.defineProperty(globalThis, 'AudioContext', {
                configurable: true,
                value: original.AudioContext,
            })
            Object.defineProperty(globalThis, 'requestAnimationFrame', {
                configurable: true,
                value: original.requestAnimationFrame,
            })
            Object.defineProperty(globalThis, 'cancelAnimationFrame', {
                configurable: true,
                value: original.cancelAnimationFrame,
            })
        },
    }
}

describe('createVoiceActivityMonitor', () => {
    it('reports speaking when the level rises and clears it after the hold', () => {
        const audio = installAudioEnvironment()
        try {
            const speaking: [string, boolean][] = []
            const monitor = createVoiceActivityMonitor({
                onSpeaking: (id, value) => speaking.push([id, value]),
            })
            monitor.watch('a', {} as MediaStream)
            const [analyser] = audio.analysers
            assert.ok(analyser)

            analyser.samples.fill(200)
            audio.tick()
            assert.deepEqual(speaking, [['a', true]])

            // Silence alone must not flicker the indicator off immediately.
            analyser.samples.fill(128)
            audio.tick(5)
            assert.deepEqual(speaking, [['a', true]])
            audio.tick(10)
            assert.deepEqual(speaking, [
                ['a', true],
                ['a', false],
            ])
            monitor.stop()
        } finally {
            audio.restore()
        }
    })

    it('meters only the streams marked as metered', () => {
        const audio = installAudioEnvironment()
        try {
            const levels: number[] = []
            const monitor = createVoiceActivityMonitor({
                onSpeaking: () => {},
                onLevel: (percent) => levels.push(percent),
            })
            monitor.watch('metered', {} as MediaStream, true)
            monitor.watch('remote', {} as MediaStream)
            for (const analyser of audio.analysers) analyser.samples.fill(200)

            audio.tick()

            assert.equal(levels.length, 1)
            assert.ok(levels[0] > 0)
            monitor.stop()
        } finally {
            audio.restore()
        }
    })

    it('reports local speech only for the metered capture stream', () => {
        // Voice activation listens to this callback, so it must describe the
        // local microphone and never a remote peer: gating the outgoing gain on
        // someone else talking would transmit at the wrong moments.
        const audio = installAudioEnvironment()
        try {
            const local: boolean[] = []
            const monitor = createVoiceActivityMonitor({
                onSpeaking: () => {},
                onLocalSpeaking: (value) => local.push(value),
            })
            monitor.watch('me', {} as MediaStream, true)
            monitor.watch('remote', {} as MediaStream)
            const [mine, theirs] = audio.analysers
            assert.ok(mine && theirs)

            // Only the remote peer is loud: the local callback must stay quiet.
            theirs.samples.fill(200)
            audio.tick(1)
            assert.deepEqual(local, [])

            // Now the local capture speaks.
            mine.samples.fill(200)
            audio.tick()
            assert.deepEqual(local, [true])

            // And silence closes it again after the hold.
            mine.samples.fill(128)
            audio.tick(15)
            assert.deepEqual(local, [true, false])
            monitor.stop()
        } finally {
            audio.restore()
        }
    })

    it('detaches a stream and clears its speaking flag when it is unwatched', () => {
        const audio = installAudioEnvironment()
        try {
            const speaking: [string, boolean][] = []
            const monitor = createVoiceActivityMonitor({
                onSpeaking: (id, value) => speaking.push([id, value]),
            })
            monitor.watch('a', {} as MediaStream)
            const [analyser] = audio.analysers
            assert.ok(analyser)
            analyser.samples.fill(200)
            audio.tick()

            monitor.watch('a', null)

            assert.deepEqual(speaking, [
                ['a', true],
                ['a', false],
            ])
            monitor.stop()
        } finally {
            audio.restore()
        }
    })

    it('stops requesting frames once the monitor is stopped', () => {
        const audio = installAudioEnvironment()
        try {
            const monitor = createVoiceActivityMonitor({ onSpeaking: () => {} })
            monitor.watch('a', {} as MediaStream)
            audio.tick()
            monitor.stop()

            assert.equal(audio.pendingFrames, 0)
        } finally {
            audio.restore()
        }
    })
})
