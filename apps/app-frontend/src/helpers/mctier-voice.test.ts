import assert from 'node:assert/strict'
import test from 'node:test'

import {
    detectNoiseSuppressionMode,
    MAX_ICE_CANDIDATE_BYTES,
    MAX_SDP_BYTES,
    MAX_SIGNAL_FRAME_BYTES,
    MctierVoiceSession,
    parseMctierVoiceSignal,
} from './mctier-voice.ts'

const FROM = 'a'.repeat(64)
const TO = 'b'.repeat(64)

type FakeTrack = {
    enabled: boolean
    label: string
    enabledWrites: number
    stops: number
    stop: () => void
}

function createStream() {
    let enabled = true
    const track: FakeTrack = {
        get enabled() {
            return enabled
        },
        set enabled(value: boolean) {
            enabled = value
            track.enabledWrites += 1
        },
        label: 'Microphone',
        enabledWrites: 0,
        stops: 0,
        stop() {
            this.stops += 1
        },
    }
    const media = {
        getTracks: () => [track],
        getAudioTracks: () => [track],
    } as unknown as MediaStream
    return { stream: media, track }
}

class FakePeerConnection {
    static instances: FakePeerConnection[] = []

    signalingState = 'stable'
    connectionState: RTCPeerConnectionState = 'new'
    iceConnectionState: RTCIceConnectionState = 'new'
    localDescription: RTCSessionDescription | null = null
    remoteDescriptions: RTCSessionDescriptionInit[] = []
    addedCandidates: RTCIceCandidateInit[] = []
    closed = false
    failRemoteDescription = false
    onicecandidate: ((event: RTCPeerConnectionIceEvent) => void) | null = null
    ontrack: ((event: RTCTrackEvent) => void) | null = null
    onconnectionstatechange: (() => void) | null = null
    oniceconnectionstatechange: (() => void) | null = null

    constructor() {
        FakePeerConnection.instances.push(this)
    }

    addTrack() {}

    async setRemoteDescription(description: RTCSessionDescriptionInit) {
        if (this.failRemoteDescription) throw new Error('remote description rejected')
        this.remoteDescriptions.push(description)
    }

    async createOffer(): Promise<RTCSessionDescriptionInit> {
        return { type: 'offer', sdp: 'offer-sdp' }
    }

    async createAnswer(): Promise<RTCSessionDescriptionInit> {
        return { type: 'answer', sdp: 'answer-sdp' }
    }

    async setLocalDescription(description: RTCSessionDescriptionInit) {
        this.localDescription = {
            ...description,
            toJSON: () => ({ ...description }),
        } as RTCSessionDescription
    }

    async addIceCandidate(candidate: RTCIceCandidateInit) {
        this.addedCandidates.push(candidate)
    }

    close() {
        this.closed = true
    }
}

type AudioStub = {
    autoplay: boolean
    muted: boolean
    volume: number
    srcObject: MediaStream | null
    dataset: Record<string, string>
    setAttribute: (name: string, value: string) => void
    setSinkId?: (deviceId: string) => Promise<void>
    play: () => Promise<void>
    pause: () => void
    remove: () => void
}

type VoiceEnvironment = {
    connections: FakePeerConnection[]
    audios: AudioStub[]
    restore: () => void
}

/** The session touches RTCPeerConnection and, only once media arrives,
 * `document.createElement('audio')`. Node has neither, so a test that wants to
 * exercise the real class has to supply both. */
function installVoiceEnvironment(): VoiceEnvironment {
    FakePeerConnection.instances = []
    const audios: AudioStub[] = []
    class TestPeerConnection extends FakePeerConnection {}
    Object.defineProperty(globalThis, 'RTCPeerConnection', {
        configurable: true,
        value: TestPeerConnection,
    })
    const documentStub = {
        createElement: () => {
            const audio: AudioStub = {
                autoplay: false,
                muted: false,
                volume: 1,
                srcObject: null,
                dataset: {},
                setAttribute: () => {},
                play: async () => {},
                pause: () => {},
                remove: () => {},
            }
            audios.push(audio)
            return audio
        },
        body: { append: () => {} },
    }
    Object.defineProperty(globalThis, 'document', { configurable: true, value: documentStub })
    return {
        connections: FakePeerConnection.instances,
        audios,
        restore: () => {
            Reflect.deleteProperty(globalThis, 'RTCPeerConnection')
            Reflect.deleteProperty(globalThis, 'document')
        },
    }
}

function receiveTrack(connection: FakePeerConnection, peerId: string) {
    const track = { kind: 'audio' } as unknown as MediaStreamTrack
    const stream = { id: `${peerId}-stream` } as unknown as MediaStream
    connection.ontrack?.({ streams: [stream], track } as unknown as RTCTrackEvent)
    return stream
}

test('accepts bounded SDP and ICE messages addressed to the local player', () => {
    assert.deepEqual(
        parseMctierVoiceSignal(
            { type: 'offer', from: FROM, to: TO, offer: { type: 'offer', sdp: 'v=0' } },
            TO,
        ),
        { type: 'offer', from: FROM, to: TO, offer: { type: 'offer', sdp: 'v=0' } },
    )
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'ice-candidate',
                from: FROM,
                to: TO,
                candidate: { candidate: 'candidate:1 1 udp 1 10.126.126.3 5000 typ host' },
            },
            TO,
        )?.type,
        'ice-candidate',
    )
})

test('rejects messages for other identities and malformed payloads', () => {
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'offer',
                from: FROM,
                to: 'c'.repeat(64),
                offer: { type: 'offer', sdp: 'v=0' },
            },
            TO,
        ),
        null,
    )
    assert.equal(
        parseMctierVoiceSignal(
            { type: 'answer', from: FROM, to: TO, answer: { type: 'answer' } },
            TO,
        ),
        null,
    )
    assert.equal(parseMctierVoiceSignal('not an object', TO), null)
})

test('rejects oversized SDP, ICE candidates and frames at the Rust limits', () => {
    // One byte past each limit the signalling host enforces.
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'offer',
                from: FROM,
                to: TO,
                offer: { type: 'offer', sdp: 'x'.repeat(MAX_SDP_BYTES + 1) },
            },
            TO,
        ),
        null,
    )
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'ice-candidate',
                from: FROM,
                to: TO,
                candidate: { candidate: 'x'.repeat(MAX_ICE_CANDIDATE_BYTES + 1) },
            },
            TO,
        ),
        null,
    )
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'offer',
                from: FROM,
                to: TO,
                offer: { type: 'offer', sdp: 'x'.repeat(MAX_SDP_BYTES) },
            },
            TO,
        )?.type,
        'offer',
    )
    // Padding outside the recognised fields still counts against the frame that
    // the signalling host measures as one message.
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'offer',
                from: FROM,
                to: TO,
                offer: { type: 'offer', sdp: 'v=0' },
                padding: 'x'.repeat(MAX_SIGNAL_FRAME_BYTES),
            },
            TO,
        ),
        null,
    )
})

test('measures signal size in UTF-8 bytes rather than UTF-16 units', () => {
    // 40k characters is under the old UTF-16 cap but 80k bytes, so only a
    // byte-accurate check rejects it.
    const multibyte = 'é'.repeat(40_000)
    assert.ok(multibyte.length < MAX_SDP_BYTES)
    assert.equal(
        parseMctierVoiceSignal(
            { type: 'offer', from: FROM, to: TO, offer: { type: 'offer', sdp: multibyte } },
            TO,
        ),
        null,
    )
})

test('negotiates an offer, buffers early candidates and applies them in order', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const sent: unknown[] = []
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async (signal) => {
                sent.push(signal)
            },
        })

        await session.syncPeers([{ playerId: TO }])
        assert.deepEqual(sent, [
            { type: 'offer', to: TO, offer: { type: 'offer', sdp: 'offer-sdp' } },
        ])
        const [connection] = environment.connections

        await session.handleSignal({
            type: 'ice-candidate',
            from: TO,
            to: FROM,
            candidate: { candidate: 'candidate:early' },
        })
        assert.deepEqual(connection.addedCandidates, [])

        await session.handleSignal({
            type: 'answer',
            from: TO,
            to: FROM,
            answer: { type: 'answer', sdp: 'answer-sdp' },
        })
        assert.deepEqual(connection.remoteDescriptions, [{ type: 'answer', sdp: 'answer-sdp' }])
        assert.deepEqual(connection.addedCandidates, [{ candidate: 'candidate:early' }])
        session.close()
    } finally {
        environment.restore()
    }
})

/** Lets an in-flight ICE-restart chain finish between simulated failures.
 *
 * `recoverPeer` deliberately ignores a `failed` report that arrives while its
 * restart offer is still in flight, because one real failure is reported on
 * both state channels and the budget must not be spent twice for it. A genuine
 * second failure can only arrive once that offer has been sent, so the fake has
 * to wait past the whole `createOffer` -> `setLocalDescription` -> `send` chain.
 * A single `await Promise.resolve()` yields one microtask, which is why the
 * later events were dropped by the in-flight guard and the restart budget was
 * never exhausted. */
function settleRestart(): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, 0))
}

test('reports a peer unreachable once its ICE restarts are exhausted', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const unreachable: string[] = []
        const sent: string[] = []
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async (signal) => {
                sent.push(signal.type)
            },
            onPeerUnreachable: (playerId) => {
                unreachable.push(playerId)
            },
        })
        // FROM sorts before TO, so this side owns the restart budget.
        await session.syncPeers([{ playerId: TO }])
        const [connection] = environment.connections
        const fail = async () => {
            connection.connectionState = 'failed'
            connection.onconnectionstatechange?.()
            await settleRestart()
        }

        // Both restarts must be spent first: reporting on either of these would
        // abandon a call that still had a recovery attempt left.
        await fail()
        assert.deepEqual(unreachable, [])
        await fail()
        assert.deepEqual(unreachable, [])

        // The third failure finds the budget exhausted and gives up on the peer.
        await fail()
        assert.deepEqual(sent, ['offer', 'offer', 'offer'])
        assert.deepEqual(unreachable, [TO])
        session.close()
    } finally {
        environment.restore()
    }
})

test('a peer that reconnects starts a fresh ICE restart budget', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const unreachable: string[] = []
        const sent: string[] = []
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async (signal) => {
                sent.push(signal.type)
            },
            onPeerUnreachable: (playerId) => {
                unreachable.push(playerId)
            },
        })
        await session.syncPeers([{ playerId: TO }])
        const [connection] = environment.connections
        const fail = async () => {
            connection.connectionState = 'failed'
            connection.onconnectionstatechange?.()
            await settleRestart()
        }
        await fail()
        await fail()
        await fail()
        assert.deepEqual(unreachable, [TO])

        // A working restart resets the budget, so the next streak has to spend
        // both restarts again instead of giving up on the first failure.
        connection.connectionState = 'connected'
        connection.onconnectionstatechange?.()
        await fail()
        await fail()
        assert.deepEqual(
            unreachable,
            [TO],
            'the reset budget must not be consumed by a single failure',
        )
        // One initial offer plus two restarts in each streak: the reconnect
        // refilled the budget in full rather than leaving it partly spent.
        assert.deepEqual(sent, ['offer', 'offer', 'offer', 'offer', 'offer'])

        // The third failure of the new streak exhausts the refilled budget and
        // reports the peer again, without offering a third restart.
        await fail()
        assert.deepEqual(unreachable, [TO, TO])
        assert.deepEqual(sent, ['offer', 'offer', 'offer', 'offer', 'offer'])
        session.close()
    } finally {
        environment.restore()
    }
})

test('a failure reported on both state channels spends only one restart', async () => {
    // A browser reports one ICE failure on both `connectionState` and
    // `iceConnectionState`, so the two handlers below describe the same event.
    // The second must not be read as "the restart that just started failed":
    // that would abandon a peer whose recovery attempt had not even been sent.
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const unreachable: string[] = []
        const sent: string[] = []
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async (signal) => {
                sent.push(signal.type)
            },
            onPeerUnreachable: (playerId) => {
                unreachable.push(playerId)
            },
        })
        await session.syncPeers([{ playerId: TO }])
        const [connection] = environment.connections
        connection.connectionState = 'failed'
        connection.onconnectionstatechange?.()
        await settleRestart()

        // One real failure, both channels, a single restart in flight.
        connection.connectionState = 'failed'
        connection.iceConnectionState = 'failed'
        connection.onconnectionstatechange?.()
        connection.oniceconnectionstatechange?.()
        await settleRestart()
        assert.deepEqual(sent, ['offer', 'offer', 'offer'])
        assert.deepEqual(unreachable, [])

        // Only a further failure once that restart settled can exhaust the
        // budget, which proves the suppression above was not a swallowed report.
        connection.connectionState = 'failed'
        connection.onconnectionstatechange?.()
        await settleRestart()
        assert.deepEqual(unreachable, [TO])
        session.close()
    } finally {
        environment.restore()
    }
})

test('reports every torn-down peer so the owner can prune its state', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const states: [string, string][] = []
        const session = new MctierVoiceSession({
            localPlayerId: TO,
            stream,
            send: async () => {},
            onPeerState: (playerId, state) => {
                states.push([playerId, state])
            },
        })

        await session.syncPeers([{ playerId: 'c'.repeat(64) }, { playerId: FROM }])
        assert.equal(environment.connections.length, 2)

        await session.syncPeers([{ playerId: 'c'.repeat(64) }])
        assert.deepEqual(states, [[FROM, 'closed']])

        session.close()
        assert.deepEqual(states, [
            [FROM, 'closed'],
            ['c'.repeat(64), 'closed'],
        ])
    } finally {
        environment.restore()
    }
})

test('stop closes every peer connection and stops the capture tracks', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream, track } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: TO,
            stream,
            send: async () => {},
        })
        await session.syncPeers([{ playerId: FROM }, { playerId: 'c'.repeat(64) }])
        assert.equal(environment.connections.length, 2)

        session.close()

        assert.ok(environment.connections.every((connection) => connection.closed))
        assert.equal(track.stops, 1)
    } finally {
        environment.restore()
    }
})

test('applies remote volume and mute to the peer audio element', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: TO,
            stream,
            send: async () => {},
        })
        await session.syncPeers([{ playerId: FROM }])
        const [connection] = environment.connections
        receiveTrack(connection, FROM)
        const [audio] = environment.audios
        assert.ok(audio)

        session.setPeerVolume(FROM, 0.5)
        session.setOutputVolume(0.5)
        assert.equal(audio.volume, 0.25)

        session.setPeerMuted(FROM, true)
        assert.equal(audio.muted, true)
        session.close()
    } finally {
        environment.restore()
    }
})

test('falls back to the default output sink when the chosen device is gone', async () => {
    const environment = installVoiceEnvironment()
    try {
        const { stream } = createStream()
        const sinkCalls: string[] = []
        let reported = 0
        const session = new MctierVoiceSession({
            localPlayerId: TO,
            stream,
            send: async () => {},
            onOutputDeviceError: () => {
                reported += 1
            },
        })
        await session.syncPeers([{ playerId: FROM }])
        receiveTrack(environment.connections[0], FROM)
        const [audio] = environment.audios
        audio.setSinkId = async (deviceId: string) => {
            sinkCalls.push(deviceId)
            if (deviceId === 'unplugged') throw new Error('no such sink')
        }

        await session.setOutputDevice('unplugged')

        assert.deepEqual(sinkCalls, ['unplugged', ''])
        assert.equal(reported, 1)
        session.close()
    } finally {
        environment.restore()
    }
})

test('does not rewrite the microphone track when the state is unchanged', () => {
    // No Web Audio in this environment, so the track itself is the switch and
    // the assertion is that a repeated call is a no-op rather than a second
    // write. The gain-gated path is covered separately below.
    const environment = installVoiceEnvironment()
    try {
        const { stream, track } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })

        session.setMicrophoneEnabled(false)
        session.setMicrophoneEnabled(false)

        assert.equal(track.enabled, false)
        assert.equal(track.enabledWrites, 1)
        session.close()
    } finally {
        environment.restore()
    }
})

/** A Web Audio stub whose gain node records every value written to it.
 *
 * Node identity is tracked so a test can assert the wiring order: RNNoise must
 * sit before the gain node, otherwise the gate's tail stays audible.
 *
 * `sampleRate` matters: the session refuses to run RNNoise unless the context
 * reports 48kHz, so the stub has to answer like a real one — honouring the
 * requested rate and otherwise using `defaultSampleRate`.
 *
 * `ignoreRequestedSampleRate` models the browsers that accept `{ sampleRate }`
 * and quietly hand back the device default anyway. Without it the stub would
 * always honour the request, and a test that only lowers `defaultSampleRate`
 * would never exercise the mismatch at all. */
function installAudioEnvironment(
    options: { defaultSampleRate?: number; ignoreRequestedSampleRate?: boolean } = {},
) {
    const defaultSampleRate = options.defaultSampleRate ?? 48000
    const gains: { value: number; writes: number[] }[] = []
    /** Directed `source -> target` edges, named so assertions read clearly. */
    const connections: string[] = []
    const previous = Object.getOwnPropertyDescriptor(globalThis, 'AudioContext')

    const named = (name: string) => ({
        name,
        connect(target: { name?: string }) {
            connections.push(`${name}->${target?.name ?? '?'}`)
        },
        disconnect() {
            connections.push(`${name}-/->`)
        },
    })

    class FakeAudioContext {
        readonly sampleRate: number

        constructor(settings?: { sampleRate?: number }) {
            this.sampleRate = options.ignoreRequestedSampleRate
                ? defaultSampleRate
                : (settings?.sampleRate ?? defaultSampleRate)
        }

        createMediaStreamSource() {
            return named('source')
        }

        createGain() {
            const entry = { value: 1, writes: [] as number[] }
            gains.push(entry)
            return {
                name: 'gain',
                get gain() {
                    return {
                        get value() {
                            return entry.value
                        },
                        set value(next: number) {
                            entry.value = next
                            entry.writes.push(next)
                        },
                    }
                },
                connect(target: { name?: string }) {
                    connections.push(`gain->${target?.name ?? '?'}`)
                },
                disconnect() {
                    connections.push('gain-/->')
                },
            }
        }

        createMediaStreamDestination() {
            const stream = createStream().stream
            return { ...named('destination'), stream }
        }

        resume() {
            return Promise.resolve()
        }

        close() {
            return Promise.resolve()
        }
    }

    Object.defineProperty(globalThis, 'AudioContext', {
        configurable: true,
        value: FakeAudioContext,
    })

    return {
        gains,
        connections,
        restore() {
            if (previous) Object.defineProperty(globalThis, 'AudioContext', previous)
            else Reflect.deleteProperty(globalThis, 'AudioContext')
        },
    }
}

test('voice activation gates the outgoing gain without touching the track', () => {
    // The whole point of the GainNode gate: the capture stays live so the local
    // analyser can still hear the user, while peers receive silence.
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    try {
        const { stream, track } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })
        const [gain] = audio.gains
        assert.ok(gain, 'a gain node should have been created')

        session.setVoiceActivationEnabled(true)
        session.setVoiceActivity(false)
        assert.equal(gain.value, 0, 'silence must gate the gain to zero')
        assert.equal(track.enabled, true, 'the capture track must stay live')

        session.setVoiceActivity(true)
        assert.equal(gain.value, 1, 'speech must reopen the gain')

        // Muting keeps using the gate, again leaving the track alone.
        session.setMicrophoneEnabled(false)
        assert.equal(gain.value, 0)
        assert.equal(track.enabled, true)

        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('open-mic mode transmits whenever the microphone is enabled', () => {
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    try {
        const { stream } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })
        const [gain] = audio.gains
        assert.ok(gain)

        // Voice activation is off by default, so activity reports must not gate
        // anything: this is the existing open-mic behaviour.
        session.setVoiceActivity(false)
        assert.equal(gain.value, 1)

        session.setMicrophoneEnabled(false)
        assert.equal(gain.value, 0)
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('defaults to the untouched source -> gain -> destination graph', () => {
    // RNNoise is opt-in, so a plain session must not construct one and must not
    // download anything: the graph is wired directly.
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    try {
        const { stream } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })

        assert.equal(session.noiseSuppressionActive, false)
        assert.deepEqual(
            audio.connections.filter((edge) => edge.startsWith('source->')),
            ['source->gain'],
        )
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('reports the capture label so a virtual microphone can be detected', () => {
    // The owner uses this to force the native path: a device that denoises on
    // its own must not be processed twice.
    const environment = installVoiceEnvironment()
    const { stream, track } = createStream()
    track.label = 'NVIDIA Broadcast'
    try {
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })

        assert.equal(session.activeCaptureLabel, 'NVIDIA Broadcast')
        session.close()
    } finally {
        environment.restore()
    }
})

test('noise suppression mode is probed without creating an audio graph', () => {
    // The probe runs before any call starts, so it must be cheap and must not
    // touch AudioContext construction.
    const previousAudioContext = Object.getOwnPropertyDescriptor(globalThis, 'AudioContext')
    try {
        Reflect.deleteProperty(globalThis, 'AudioContext')
        assert.equal(detectNoiseSuppressionMode(), 'unavailable')
    } finally {
        if (previousAudioContext) {
            Object.defineProperty(globalThis, 'AudioContext', previousAudioContext)
        }
    }
})

test('falls back to native when the worklet assets cannot load', async () => {
    // This is a guaranteed failure in `node --test`, for two independent
    // reasons, both verified against the installed package: its entry module
    // evaluates `class ... extends AudioWorkletNode` at import time and that
    // global only exists in a browser, and the `?url` specifiers are a Vite
    // transform that Node's resolver rejects with ERR_PACKAGE_PATH_NOT_EXPORTED.
    // Whichever rejects first, enabling must report `false` and the call must
    // keep working. The failure is asserted rather than assumed: if the loader
    // ever started succeeding here, `started` would be true and this would fail.
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    try {
        const { stream, track } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })

        const started = await session.setNoiseSuppressionEnabled(true)

        assert.equal(started, false)
        assert.equal(session.noiseSuppressionActive, false)
        // The original path is still intact, so audio keeps flowing unfiltered.
        assert.deepEqual(
            audio.connections.filter((edge) => edge.endsWith('->gain')),
            ['source->gain'],
        )
        assert.equal(track.enabled, true)
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('turning noise suppression off disconnects the worklet', async () => {
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    try {
        const { stream } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
        })
        // Enable fails here (no assets), which is enough to prove the off path
        // stays safe and leaves no dangling node behind.
        await session.setNoiseSuppressionEnabled(true)
        await session.setNoiseSuppressionEnabled(false)

        assert.equal(session.noiseSuppressionActive, false)
        assert.equal(audio.gains.length, 1)
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('refuses RNNoise when the audio graph is not at 48kHz', async () => {
    // RNNoise cannot detect a rate mismatch: its frame size is a wasm constant,
    // so at 44.1kHz it would process 480 samples as if they were 10ms and
    // pitch-shift the voice by ~8.8%. The session has to refuse instead.
    const environment = installVoiceEnvironment()
    // `ignoreRequestedSampleRate` is what models the real failure: the product
    // asks for 48kHz and the browser hands back 44.1kHz without complaining.
    const audio = installAudioEnvironment({
        defaultSampleRate: 44100,
        ignoreRequestedSampleRate: true,
    })
    let loaderCalls = 0
    try {
        const { stream, track } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
            // A loader that would happily hand back a node: proving the refusal
            // comes from the rate check, not from a load failure.
            loadNoiseNode: async () => {
                loaderCalls += 1
                return { connect() {}, disconnect() {} } as unknown as AudioWorkletNode
            },
        })

        assert.equal(session.supportsNoiseSuppression, false)
        const started = await session.setNoiseSuppressionEnabled(true)

        assert.equal(started, false)
        assert.equal(session.noiseSuppressionActive, false)
        // Loading must not even be attempted: downloading ~217 KB for a node
        // that would be discarded is wasted bandwidth.
        assert.equal(loaderCalls, 0)
        // Voice itself is unaffected: the plain graph is still wired and the
        // capture track is still live.
        assert.deepEqual(
            audio.connections.filter((edge) => edge.endsWith('->gain')),
            ['source->gain'],
        )
        assert.equal(track.enabled, true)
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})

test('a disable during loading wins over the enable that arrives late', async () => {
    // The switch is a user action, not a queue entry: if it is turned off while
    // the worklet is still downloading, the late arrival must be thrown away
    // rather than connected to a graph the user believes is off.
    const environment = installVoiceEnvironment()
    const audio = installAudioEnvironment()
    let releaseLoad!: () => void
    let signalEntered!: () => void
    const enteredLoading = new Promise<void>((resolve) => {
        signalEntered = resolve
    })
    let destroys = 0
    try {
        const { stream } = createStream()
        const session = new MctierVoiceSession({
            localPlayerId: FROM,
            stream,
            send: async () => {},
            loadNoiseNode: async () => {
                // Announce that the load is in flight, then hold it open until
                // the test lets it finish, so the disable lands mid-await.
                signalEntered()
                await new Promise<void>((resolve) => {
                    releaseLoad = resolve
                })
                return {
                    connect() {},
                    disconnect() {},
                    destroy() {
                        destroys += 1
                    },
                } as unknown as AudioWorkletNode
            },
        })

        const enabling = session.setNoiseSuppressionEnabled(true)
        await enteredLoading
        await session.setNoiseSuppressionEnabled(false)
        releaseLoad()
        const started = await enabling

        assert.equal(started, false)
        assert.equal(session.noiseSuppressionActive, false)
        // The discarded node was released, not left holding a wasm heap.
        assert.equal(destroys, 1)
        // And it never entered the graph.
        assert.deepEqual(
            audio.connections.filter((edge) => edge.endsWith('->gain')),
            ['source->gain'],
        )
        session.close()
    } finally {
        audio.restore()
        environment.restore()
    }
})
