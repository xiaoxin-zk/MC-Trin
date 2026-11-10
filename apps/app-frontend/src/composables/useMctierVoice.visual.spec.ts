import { afterEach, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import { MAX_ICE_CANDIDATE_BYTES, MAX_SDP_BYTES } from '../helpers/mctier-voice'
import { useMctierVoice } from './useMctierVoice'

const rtcOperations: string[] = []
const connections: FakeRTCPeerConnection[] = []

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

    constructor() {
        connections.push(this)
    }

    addTrack() {}

    async setRemoteDescription(description: RTCSessionDescriptionInit) {
        rtcOperations.push(`remote:${description.type}`)
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
        voice_ice_server: null,
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

    expect(track.stop).toHaveBeenCalledOnce()
    expect(voice.active.value).toBe(false)
    expect(voice.starting.value).toBe(false)
})

it('enables the microphone only while push-to-talk is held', async () => {
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()

    voice.setPushToTalk(true)
    voice.syncRoom(state)
    await voice.start(state)

    expect(track.enabled).toBe(false)

    voice.setPttHeld(true)
    expect(track.enabled).toBe(true)
    // Repeating the same state must not keep poking the live track.
    const writesWhileHeld = track.enabledWrites
    voice.setPttHeld(true)
    expect(track.enabledWrites).toBe(writesWhileHeld)

    voice.setPttHeld(false)
    expect(track.enabled).toBe(false)

    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'F2' }))
    expect(track.enabled).toBe(true)
    window.dispatchEvent(new KeyboardEvent('keyup', { code: 'F2' }))
    expect(track.enabled).toBe(false)

    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'F2' }))
    expect(track.enabled).toBe(true)
    window.dispatchEvent(new Event('blur'))
    expect(track.enabled).toBe(false)
})

it('re-enables a persisted push-to-talk microphone on the next session', async () => {
    localStorage.setItem(
        'mctier.voice.preferences',
        JSON.stringify({ pushToTalk: true, pushToTalkKey: 'F2' }),
    )
    const { stream, track } = createStream()
    stubGetUserMedia(vi.fn(async () => stream))
    vi.stubGlobal('RTCPeerConnection', FakeRTCPeerConnection)
    const { voice } = createVoice()
    const state = roomState()

    voice.syncRoom(state)
    await voice.start(state)
    expect(track.enabled).toBe(false)

    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'F2' }))
    expect(track.enabled).toBe(true)
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
