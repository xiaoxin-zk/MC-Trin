/** Speaking detection and microphone level metering for voice chat, ported
 * from MCTier's SpeakingDetector: every watched stream gets one AnalyserNode
 * fed by its raw capture, and a shared requestAnimationFrame loop turns the
 * time-domain samples into a speaking flag (with a short silence hold so the
 * indicator does not flicker) and, for metered streams, a 0-100 level. */

const SPEAKING_RMS_THRESHOLD = 0.02
const SILENCE_HOLD_FRAMES = 12
const LEVEL_FLOOR_DB = -60

/** Root mean square of byte-shaped time-domain samples centred on 128. */
export function byteRms(samples: Uint8Array): number {
    let sum = 0
    for (let index = 0; index < samples.length; index += 1) {
        const value = (samples[index] - 128) / 128
        sum += value * value
    }
    return samples.length ? Math.sqrt(sum / samples.length) : 0
}

/** Maps an RMS value onto 0-100 with a -60 dBFS floor, so quiet rooms show
 * movement without pinning the meter at the bottom. */
export function microphoneLevelPercent(rms: number): number {
    if (!Number.isFinite(rms) || rms <= 0) return 0
    const decibels = 20 * Math.log10(rms)
    return Math.round(
        Math.max(0, Math.min(100, ((decibels - LEVEL_FLOOR_DB) / -LEVEL_FLOOR_DB) * 100)),
    )
}

type WatchedStream = {
    source: MediaStreamAudioSourceNode
    analyser: AnalyserNode
    samples: Uint8Array
    speaking: boolean
    silentFrames: number
    metered: boolean
}

export type VoiceActivityMonitor = {
    /** Starts analysing a stream; passing null detaches it. Metered streams
     * additionally report a level percentage on every frame. */
    watch: (id: string, stream: MediaStream | null, metered?: boolean) => void
    stop: () => void
}

export function createVoiceActivityMonitor(callbacks: {
    onSpeaking: (id: string, speaking: boolean) => void
    onLevel?: (percent: number) => void
}): VoiceActivityMonitor {
    const watched = new Map<string, WatchedStream>()
    const AudioContextConstructor = globalThis.AudioContext
    let context: AudioContext | null = null
    let frame = 0

    function ensureContext(): AudioContext | null {
        if (context || !AudioContextConstructor) return context
        try {
            context = new AudioContextConstructor()
        } catch {
            context = null
        }
        return context
    }

    function detach(id: string) {
        const entry = watched.get(id)
        if (!entry) return
        watched.delete(id)
        try {
            entry.source.disconnect()
            entry.analyser.disconnect()
        } catch {
            // The stream or context may already be gone.
        }
        if (entry.speaking) callbacks.onSpeaking(id, false)
    }

    function tick() {
        for (const [id, entry] of watched) {
            entry.analyser.getByteTimeDomainData(entry.samples)
            const rms = byteRms(entry.samples)
            if (rms >= SPEAKING_RMS_THRESHOLD) {
                entry.silentFrames = 0
                if (!entry.speaking) {
                    entry.speaking = true
                    callbacks.onSpeaking(id, true)
                }
            } else if (entry.speaking) {
                entry.silentFrames += 1
                if (entry.silentFrames >= SILENCE_HOLD_FRAMES) {
                    entry.speaking = false
                    callbacks.onSpeaking(id, false)
                }
            }
            if (entry.metered) callbacks.onLevel?.(microphoneLevelPercent(rms))
        }
        frame = requestAnimationFrame(tick)
    }

    return {
        watch(id, stream, metered = false) {
            detach(id)
            if (!stream) return
            const audioContext = ensureContext()
            if (!audioContext) return
            try {
                const source = audioContext.createMediaStreamSource(stream)
                const analyser = audioContext.createAnalyser()
                analyser.fftSize = 512
                analyser.smoothingTimeConstant = 0.5
                source.connect(analyser)
                watched.set(id, {
                    source,
                    analyser,
                    samples: new Uint8Array(analyser.fftSize),
                    speaking: false,
                    silentFrames: 0,
                    metered,
                })
                if (!frame) frame = requestAnimationFrame(tick)
            } catch {
                // A stream that cannot be analysed simply reports nothing.
            }
        },
        stop() {
            for (const id of [...watched.keys()]) detach(id)
            if (frame) {
                cancelAnimationFrame(frame)
                frame = 0
            }
            if (context) {
                void context.close().catch(() => {})
                context = null
            }
        },
    }
}
