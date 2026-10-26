import { afterEach, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import { useMctierVoice } from './useMctierVoice'

const rtcOperations: string[] = []

class FakeRTCPeerConnection {
    signalingState = 'stable'
    connectionState = 'new'
    iceConnectionState = 'new'
    localDescription: RTCSessionDescription | null = null
    onicecandidate: ((event: RTCPeerConnectionIceEvent) => void) | null = null
    ontrack: ((event: RTCTrackEvent) => void) | null = null
    onconnectionstatechange: (() => void) | null = null
    oniceconnectionstatechange: (() => void) | null = null

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

    close() {}
}

function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => {
        resolve = done
    })
    return { promise, resolve }
}

function createStream() {
    const track = { enabled: true, stop: vi.fn() }
    const stream = {
        getTracks: () => [track],
        getAudioTracks: () => [track],
    } as unknown as MediaStream
    return { stream, track }
}

function roomState(localPlayerId = 'z'): MctierState {
    return {
        status: 'hosting',
        mode: 'create',
        local_player_id: localPlayerId,
        players: [{ playerId: 'a', playerName: 'Player A', virtualIp: null, sessionGeneration: 1 }],
        voice_ice_server: null,
    } as MctierState
}

let originalMediaDevices: PropertyDescriptor | undefined
const cleanups: (() => void)[] = []

function stubGetUserMedia(getUserMedia: MediaDevices['getUserMedia']) {
    originalMediaDevices = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices')
    Object.defineProperty(navigator, 'mediaDevices', {
        configurable: true,
        value: { getUserMedia },
    })
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
    if (originalMediaDevices) {
        Object.defineProperty(navigator, 'mediaDevices', originalMediaDevices)
    } else {
        Reflect.deleteProperty(navigator, 'mediaDevices')
    }
    originalMediaDevices = undefined
    vi.unstubAllGlobals()
    rtcOperations.length = 0
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
