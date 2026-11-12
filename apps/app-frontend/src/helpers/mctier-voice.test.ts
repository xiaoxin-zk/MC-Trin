import assert from 'node:assert/strict'
import test from 'node:test'

import {
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

/** A Web Audio stub whose gain node records every value written to it. */
function installAudioEnvironment() {
    const gains: { value: number; writes: number[] }[] = []
    const previous = Object.getOwnPropertyDescriptor(globalThis, 'AudioContext')

    class FakeAudioContext {
        createMediaStreamSource() {
            return { connect() {}, disconnect() {} }
        }

        createGain() {
            const entry = { value: 1, writes: [] as number[] }
            gains.push(entry)
            return {
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
                connect() {},
                disconnect() {},
            }
        }

        createMediaStreamDestination() {
            const stream = createStream().stream
            return { stream, connect() {}, disconnect() {} }
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
