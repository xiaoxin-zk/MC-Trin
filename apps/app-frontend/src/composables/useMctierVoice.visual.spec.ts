import { flushPromises } from '@vue/test-utils'
import { afterEach, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import { MAX_ICE_CANDIDATE_BYTES, MAX_SDP_BYTES } from '../helpers/mctier-voice'
import { useMctierVoice } from './useMctierVoice'

const rtcOperations: string[] = []
const connections: FakeRTCPeerConnection[] = []
const rtcConfigurations: RTCConfiguration[] = []

class FakeRTCPeerConnection {
    signalingState = 'stable'
    connectionState = 'new'
    iceConnectionState = 'new'
    localDescription: RTCSessionDescription | null = null
    onicecandidate: ((event: RTCPeerConnectionIceEvent) => void) | null = null
    ontrack: ((event: RTCTrackEvent) => void) | null = null
    onconnectionstatechange: (() => void) | null = null
    oniceconnectionstatechange: (() => void) | null = null
    closed = false

    constructor(configuration?: RTCConfiguration) {
        rtcConfigurations.push(configuration ?? {})
        connections.push(this)
    }

    addTrack() {}

    async setRemoteDescription(description: RTCSessionDescriptionInit) {
        rtcOperations.push(`remote:${description.type}`)
    }

    async createOffer(): Promise<RTCSessionDescriptionInit> {
        return { type: 'offer', sdp: 'offer-sdp' }
    }

    async createAnswer(): Promise<RTCSessionDescriptionInit> {
        return { type: 'answer', sdp: 'answer-sdp' }
    }

    async setLocalDescription(description: RTCSessionDescriptionInit) {
        const stored = { ...description, toJSON: () => ({ ...description }) }
        this.localDescription = stored as RTCSessionDescription
        rtcOperations.push(`local:${description.type}`)
    }

    async addIceCandidate(candidate: RTCIceCandidateInit) {
        rtcOperations.push(`candidate:${candidate.candidate}`)
    }

    /** Mimics the browser firing a state change so peer bookkeeping can be
     * asserted without a real network. */
    setConnectionState(state: RTCPeerConnectionState) {
        this.connectionState = state
        this.onconnectionstatechange?.()
    }

    close() {
        this.closed = true
    }
}

type FakeTrack = {
    enabled: boolean
    enabledWrites: number
    label: string
    readyState: string
    stopped: boolean
    onended: ((event: Event) => void) | null
    stop: () => void
    /** Present because the real `MediaStreamTrack` always has it: a virtual
     * microphone label makes `requestMicrophone` re-apply the processing
     * constraints, and a missing method would throw before the session starts. */
    applyConstraints: (constraints?: MediaTrackConstraints) => Promise<void>
    appliedConstraints: MediaTrackConstraints[]
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
        enabledWrites: 0,
        label: 'Microphone',
        readyState: 'live',
        stopped: false,
        onended: null,
        stop() {
            track.stopped = true
            track.readyState = 'ended'
        },
        appliedConstraints: [],
        applyConstraints(constraints?: MediaTrackConstraints) {
            track.appliedConstraints.push(constraints ?? {})
            return Promise.resolve()
        },
    }
    const stream = {
        getTracks: () => [track],
        getAudioTracks: () => [track],
    } as unknown as MediaStream
    return { stream, track }
}

function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => {
        resolve = done
    })
    return { promise, resolve }
}

function roomState(localPlayerId = 'z', playerIds = ['a']): MctierState {
    return {
        status: 'hosting',
        mode: 'create',
        local_player_id: localPlayerId,
        players: playerIds.map((playerId) => ({
            playerId,
            playerName: `Player ${playerId}`,
            virtualIp: null,
            sessionGeneration: 1,
        })),
    } as MctierState
}

let originalMediaDevices: PropertyDescriptor | undefined
const cleanups: (() => void)[] = []

function stubGetUserMedia(
    getUserMedia: MediaDevices['getUserMedia'],
    devices: MediaDeviceInfo[] = [],
) {
    originalMediaDevices = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices')
    Object.defineProperty(navigator, 'mediaDevices', {
        configurable: true,
        value: {
            getUserMedia,
            enumerateDevices: vi.fn(async () => devices),
            // The real `MediaDevices` always exposes this in Chromium, and the
            // session probes it per key before sending any constraint. Without
            // it the probe falls back to `{}`, the capture ends up with no
            // constraints at all, and tests about processed-vs-raw input would
            // pass vacuously. Booleans mean "this constraint is supported".
            getSupportedConstraints: () => ({
                autoGainControl: true,
                echoCancellation: true,
                noiseSuppression: true,
            }),
            addEventListener: vi.fn(),
            removeEventListener: vi.fn(),
        },
    })
}

function device(kind: MediaDeviceKind, deviceId: string, label: string): MediaDeviceInfo {
    return { kind, deviceId, label, groupId: 'group', toJSON: () => ({}) } as MediaDeviceInfo
}

function createVoice(send = vi.fn(async () => {})) {
    const scope = effectScope()
    const voice = scope.run(() => useMctierVoice(send))!
    cleanups.push(() => {
        voice.stop()
        scope.stop()
    })
    return { voice, send }
}

afterEach(() => {
    for (const cleanup of cleanups.splice(0)) cleanup()
    localStorage.removeItem('mctier.voice.preferences')
    if (originalMediaDevices) {
        Object.defineProperty(navigator, 'mediaDevices', originalMediaDevices)
    } else {
        Reflect.deleteProperty(navigator, 'mediaDevices')
    }
    originalMediaDevices = undefined
    vi.unstubAllGlobals()
    rtcOperations.length = 0
    connections.length = 0
    rtcConfigurations.length = 0
})

it('replays offer and ICE signals received before the user enables voice', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice, send } = createVoice()
    const state = roomState()
    const offer = {
        type: 'offer',
        from: 'a',
        to: 'z',
        offer: { type: 'offer', sdp: 'offer-sdp' },
    }
    const candidate = {
        type: 'ice-candidate',
        from: 'a',
        to: 'z',
        candidate: { candidate: 'candidate:one' },
    }

    voice.syncRoom(state)
    await voice.receiveSignal(offer)
    await voice.receiveSignal(candidate)
    await voice.start(state)

    expect(rtcOperations).toEqual(['remote:offer', 'local:answer', 'candidate:candidate:one'])
    expect(send).toHaveBeenCalledWith({
        type: 'answer',
        to: 'a',
        answer: { type: 'answer', sdp: 'answer-sdp' },
    })
    expect(voice.active.value).toBe(true)
})

it('bounds queued signaling and retains the newest signals in arrival order', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()

    voice.syncRoom(state)
    for (let index = 1; index <= 130; index += 1) {
        await voice.receiveSignal({
            type: 'ice-candidate',
            from: 'a',
            to: 'z',
            candidate: { candidate: `candidate:${index}` },
        })
    }
    await voice.receiveSignal({
        type: 'offer',
        from: 'a',
        to: 'z',
        offer: { type: 'offer', sdp: 'offer-sdp' },
    })
    await voice.start(state)

    expect(rtcOperations.filter((operation) => operation.startsWith('candidate:'))).toEqual(
        Array.from({ length: 127 }, (_, index) => `candidate:candidate:${index + 4}`),
    )
})

it('invalidates a pending microphone request and stops a stream that resolves late', async () => {
    const request = deferred<MediaStream>()
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(() => request.promise))
    const { voice } = createVoice()
    const start = voice.start(roomState('solo'))

    expect(voice.starting.value).toBe(true)
    voice.stop()
    expect(voice.starting.value).toBe(false)

    request.resolve(stream)
    await start

    expect(track.stopped).toBe(true)
    expect(track.readyState).toBe('ended')
    expect(voice.active.value).toBe(false)
    expect(voice.starting.value).toBe(false)
})

it('opens on open-mic and only mutes when the user mutes', async () => {
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()

    voice.syncRoom(state)
    await voice.start(state)

    // Voice activation is off by default, so the microphone transmits.
    expect(track.enabled).toBe(true)

    voice.toggleMute()
    expect(track.enabled).toBe(false)

    voice.toggleMute()
    expect(track.enabled).toBe(true)
})

it('remembers voice activation across sessions', async () => {
    localStorage.setItem('mctier.voice.preferences', JSON.stringify({ voiceActivation: true }))
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()

    expect(voice.voiceActivation.value).toBe(true)

    // Toggling persists the choice for the next session.
    voice.setVoiceActivation(false)
    expect(
        JSON.parse(localStorage.getItem('mctier.voice.preferences') ?? '{}').voiceActivation,
    ).toBe(false)
})

it('defaults noise reduction off and never persists the mode string', () => {
    // The stored preference is a boolean the user owns. The resolved mode is a
    // property of the machine and is re-detected, so persisting it would let a
    // stale value promise a mode this webview cannot run.
    const { voice } = createVoice()

    expect(voice.noiseReduction.value).toBe(false)
    expect(['native', 'rnnoise', 'unavailable']).toContain(voice.noiseSuppressionMode.value)

    voice.setNoiseReduction(true)
    const stored = JSON.parse(localStorage.getItem('mctier.voice.preferences') ?? '{}')
    expect('noiseSuppressionMode' in stored).toBe(false)
    expect('noiseReductionMode' in stored).toBe(false)
    // The only noise key is the boolean, and it is true only when the mode can
    // actually honour it.
    if (stored.noiseReduction !== undefined) {
        expect(voice.noiseSuppressionMode.value).toBe('rnnoise')
        expect(stored.noiseReduction).toBe(true)
    }
})

it('forces the native path when the capture device is a virtual microphone', async () => {
    // A device that already denoises must not be processed twice, so the switch
    // is pinned to native regardless of the stored preference.
    localStorage.setItem('mctier.voice.preferences', JSON.stringify({ noiseReduction: true }))
    const { stream, track } = createStream()
    track.label = 'Krisp Audio'
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()

    voice.syncRoom(state)
    await voice.start(state)
    // `applyNoiseReduction` is deliberately not awaited by `start` (it would
    // block the call on a ~217 KB download), so wait on the state it publishes
    // rather than on a timeout. `noiseSuppressionMode` settling to 'native' is
    // exactly the last write that function makes on this path.
    await vi.waitFor(() => expect(voice.noiseSuppressionMode.value).toBe('native'))

    // The virtual microphone was detected from the capture label, which is what
    // pins the mode; asserting both keeps the test honest if the label lookup
    // regresses and 'native' starts coming from the rate check instead.
    expect(voice.noiseReductionForcedNative.value).toBe(true)
    // Re-applying the constraints is the observable proof that the virtual-mic
    // branch actually ran during capture setup.
    expect(track.appliedConstraints).toHaveLength(1)
    expect(track.appliedConstraints[0]).toMatchObject({ noiseSuppression: false })
})

it('hides noise reduction when the webview cannot run it', () => {
    const previous = Object.getOwnPropertyDescriptor(globalThis, 'AudioContext')
    try {
        Reflect.deleteProperty(globalThis, 'AudioContext')
        const { voice } = createVoice()
        expect(voice.noiseSuppressionMode.value).toBe('unavailable')
    } finally {
        if (previous) Object.defineProperty(globalThis, 'AudioContext', previous)
    }
})

it('drops a peer from the connected count and closes its connection when it leaves', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    voice.syncRoom(roomState('z', ['a', 'b']))
    await voice.start(roomState('z', ['a', 'b']))
    expect(connections).toHaveLength(2)

    for (const connection of connections) connection.setConnectionState('connected')
    expect(voice.connectedPeers.value).toBe(2)

    voice.syncRoom(roomState('z', ['a']))

    expect(voice.connectedPeers.value).toBe(1)
    expect(connections.filter((connection) => connection.closed)).toHaveLength(1)
})

it('uses only the public STUN servers for ICE', async () => {
    // The launcher used to prepend a "stun:" server on EasyTier's 10.126.126.x
    // overlay address. EasyTier runs with --no-tun/--use-smoltcp, so no adapter
    // owns that address and the bind failed on every machine; the only honest
    // configuration is the two public servers.
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState('a', ['z'])
    voice.syncRoom(state)
    await voice.start(state)

    expect(rtcConfigurations).toHaveLength(1)
    expect(rtcConfigurations[0].iceServers).toEqual([
        { urls: 'stun:stun.qq.com:3478' },
        { urls: 'stun:stun.miwifi.com:3478' },
    ])
    expect(JSON.stringify(rtcConfigurations[0])).not.toContain('10.126.126')
})

it('reports a failed call instead of waiting forever when ICE gives up', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    // 'a' sorts before 'z', so this side is the offering side and owns the ICE
    // restart budget. The answering side never restarts and never abandons.
    const state = roomState('a', ['z'])
    voice.syncRoom(state)
    await voice.start(state)
    expect(voice.iceFailed.value).toBe(false)

    const peer = connections.find((candidate) => !candidate.closed)!
    for (let attempt = 0; attempt <= 2; attempt += 1) {
        peer.setConnectionState('failed')
        await flushPromises()
    }

    expect(voice.iceFailed.value).toBe(true)
    // The state is a failed call, not a call waiting for somebody to join.
    expect(voice.connectedPeers.value).toBe(0)
})

it('clears the failed state when the retried call reaches a peer', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState('a', ['z'])
    voice.syncRoom(state)
    await voice.start(state)
    const peer = connections.find((candidate) => !candidate.closed)!
    for (let attempt = 0; attempt <= 2; attempt += 1) {
        peer.setConnectionState('failed')
        await flushPromises()
    }
    expect(voice.iceFailed.value).toBe(true)

    await voice.retry()

    expect(voice.iceFailed.value).toBe(false)
    expect(voice.active.value).toBe(true)
})

it('drops a peer from the failed set once it connects', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState('a', ['z'])
    voice.syncRoom(state)
    await voice.start(state)
    const peer = connections.find((candidate) => !candidate.closed)!
    for (let attempt = 0; attempt <= 2; attempt += 1) {
        peer.setConnectionState('failed')
        await flushPromises()
    }
    expect(voice.iceFailed.value).toBe(true)

    peer.setConnectionState('connected')

    expect(voice.iceFailed.value).toBe(false)
    expect(voice.connectedPeers.value).toBe(1)
})

it('does not offer a full retry while another peer is still connected', async () => {
    // Two peers where one is talking and one exhausted its ICE restarts. The
    // retry action closes *every* peer and rebuilds the session, so offering it
    // here would destroy a working call to retry one unreachable participant.
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    // 'a' sorts before both peers, so this side offers and owns the restart
    // budget for each of them.
    const state = roomState('a', ['b', 'c'])
    voice.syncRoom(state)
    await voice.start(state)
    expect(connections).toHaveLength(2)

    const [doomed, healthy] = connections
    for (let attempt = 0; attempt <= 2; attempt += 1) {
        doomed.setConnectionState('failed')
        await flushPromises()
    }
    healthy.setConnectionState('connected')

    expect(voice.connectedPeers.value).toBe(1)
    expect(voice.iceFailed.value).toBe(true)
    // Partial failure: the notice is reported but no destructive action is
    // offered, and the guard in `retry()` keeps a stray click from tearing the
    // working peer down too.
    expect(voice.voiceUnreachable.value).toBe(false)
    expect(voice.canRetry.value).toBe(false)
    expect(voice.unreachablePeerCount.value).toBe(1)

    const survivors = connections.filter((connection) => !connection.closed)
    expect(survivors).toHaveLength(2)
    await voice.retry()
    expect(connections.filter((connection) => connection.closed)).toHaveLength(0)
    expect(voice.active.value).toBe(true)
    expect(voice.connectedPeers.value).toBe(1)
})

it('offers a full retry only when no peer is connected', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState('a', ['z'])
    voice.syncRoom(state)
    await voice.start(state)
    const peer = connections.find((candidate) => !candidate.closed)!
    for (let attempt = 0; attempt <= 2; attempt += 1) {
        peer.setConnectionState('failed')
        await flushPromises()
    }

    expect(voice.connectedPeers.value).toBe(0)
    expect(voice.voiceUnreachable.value).toBe(true)
    expect(voice.canRetry.value).toBe(true)
})

it('closes every peer connection and stops the capture track when voice stops', async () => {
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState('z', ['a', 'b'])
    voice.syncRoom(state)
    await voice.start(state)
    expect(connections).toHaveLength(2)

    voice.stop()

    expect(connections.every((connection) => connection.closed)).toBe(true)
    expect(track.stopped).toBe(true)
    expect(voice.active.value).toBe(false)
    expect(voice.connectedPeers.value).toBe(0)
})

it('rejects signals above the signalling limits instead of sending them', async () => {
    const { stream } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice, send } = createVoice()
    const state = roomState()

    voice.syncRoom(state)
    await voice.receiveSignal({
        type: 'offer',
        from: 'a',
        to: 'z',
        offer: { type: 'offer', sdp: 'x'.repeat(MAX_SDP_BYTES + 1) },
    })
    await voice.receiveSignal({
        type: 'ice-candidate',
        from: 'a',
        to: 'z',
        candidate: { candidate: 'x'.repeat(MAX_ICE_CANDIDATE_BYTES + 1) },
    })
    await voice.start(state)

    expect(rtcOperations).toEqual([])
    expect(send).not.toHaveBeenCalled()
})

it('keeps the working call when the chosen microphone fails to open', async () => {
    const { stream, track } = createStream()
    const getUserMedia = vi.fn(async (constraints: MediaStreamConstraints) => {
        const audio = constraints.audio as MediaTrackConstraints | undefined
        if (audio?.deviceId && (audio.deviceId as { exact: string }).exact === 'broken') {
            throw new DOMException('device busy', 'NotReadableError')
        }
        return stream
    })
    stubGetUserMedia(getUserMedia as unknown as MediaDevices['getUserMedia'], [
        device('audioinput', 'good', 'Good mic'),
        device('audioinput', 'broken', 'Busy mic'),
    ])
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()
    voice.syncRoom(state)
    await voice.start(state)
    expect(voice.active.value).toBe(true)

    await voice.setInputDevice('broken')

    expect(voice.active.value).toBe(true)
    expect(track.stopped).toBe(false)
    // The refused choice is not committed, and the live call stays healthy.
    expect(voice.inputDeviceId.value).toBeNull()
    expect(voice.error.value).toBeNull()
})

it('stops a one-way call when the capture device disappears', async () => {
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()
    voice.syncRoom(state)
    await voice.start(state)
    expect(voice.active.value).toBe(true)

    track.readyState = 'ended'
    track.onended?.(new Event('ended'))

    expect(voice.active.value).toBe(false)
    expect(voice.error.value).toBe('input-device-lost')
})

it('clears a finished call error when the next room starts', async () => {
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    voice.syncRoom(roomState())
    await voice.start(roomState())
    track.readyState = 'ended'
    track.onended?.(new Event('ended'))
    expect(voice.error.value).toBe('input-device-lost')

    // The panel is always on screen, so a complaint about the previous room has
    // to go away once a new room lifecycle begins.
    voice.syncRoom({ ...roomState(), status: 'connecting' })

    expect(voice.error.value).toBeNull()
})
