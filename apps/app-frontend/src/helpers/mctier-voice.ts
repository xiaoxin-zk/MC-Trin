export type MctierVoiceSignal = {
    type: 'offer' | 'answer' | 'ice-candidate'
    from?: string
    to: string
    offer?: RTCSessionDescriptionInit
    answer?: RTCSessionDescriptionInit
    candidate?: RTCIceCandidateInit
}

type PeerConnectionState = {
    connection: RTCPeerConnection
    remoteDescriptionSet: boolean
    pendingCandidates: RTCIceCandidateInit[]
    makingOffer: boolean
    audio: HTMLAudioElement | null
}

export type VoicePeer = { playerId: string }
export type VoiceConnectionState = RTCPeerConnectionState | RTCIceConnectionState

export type MctierVoiceSessionOptions = {
    localPlayerId: string
    stream: MediaStream
    iceServer?: string | null
    send: (signal: MctierVoiceSignal) => Promise<void>
    onPeerState?: (playerId: string, state: VoiceConnectionState) => void
    /** Hands each remote playback stream to the caller (and null on detach)
     * so speaking detection can attach analysers outside the session. */
    onRemoteStream?: (playerId: string, stream: MediaStream | null) => void
    /** Fires when the chosen sink could not be applied and playback fell back
     * to the system default, so the owner can stop advertising the dead device. */
    onOutputDeviceError?: () => void
}

/** These mirror the signalling frame limits in
 * `packages/app-lib/src/api/mctier/protocol.rs`; a signal accepted here but
 * rejected there fails the whole send, so the two must not drift. */
export const MAX_SIGNAL_FRAME_BYTES = 128 * 1024
export const MAX_SDP_BYTES = 64 * 1024
export const MAX_ICE_CANDIDATE_BYTES = 4 * 1024

const MAX_PENDING_CANDIDATES = 128
const DEFAULT_OUTPUT_VOLUME = 1
const DEFAULT_INPUT_VOLUME = 1
const DEFAULT_PEER_VOLUME = 1

function utf8Length(value: string): number {
    return new TextEncoder().encode(value).length
}

/** Playback volume is stored as a 0-1 gain so the slider and the element agree. */
function clampVolume(volume: number): number {
    if (!Number.isFinite(volume)) return DEFAULT_OUTPUT_VOLUME
    return Math.min(1, Math.max(0, volume))
}

type SinkCapableAudio = HTMLAudioElement & {
    setSinkId?: (deviceId: string) => Promise<void>
}

export function parseMctierVoiceSignal(
    value: unknown,
    localPlayerId: string,
): MctierVoiceSignal | null {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null
    const candidate = value as Record<string, unknown>
    if (
        candidate.to !== localPlayerId ||
        typeof candidate.from !== 'string' ||
        !candidate.from ||
        candidate.from === localPlayerId ||
        !['offer', 'answer', 'ice-candidate'].includes(String(candidate.type))
    ) {
        return null
    }
    try {
        if (utf8Length(JSON.stringify(value) ?? '') > MAX_SIGNAL_FRAME_BYTES) return null
    } catch {
        return null
    }

    if (candidate.type === 'offer' || candidate.type === 'answer') {
        const description = candidate[candidate.type] as Record<string, unknown> | null
        const sdp = description?.sdp
        if (
            !description ||
            Array.isArray(description) ||
            typeof sdp !== 'string' ||
            !sdp ||
            utf8Length(sdp) > MAX_SDP_BYTES
        ) {
            return null
        }
    }
    if (candidate.type === 'ice-candidate') {
        const ice = candidate.candidate
        const iceValue =
            ice && typeof ice === 'object' && !Array.isArray(ice)
                ? (ice as Record<string, unknown>).candidate
                : undefined
        if (
            !ice ||
            typeof ice !== 'object' ||
            Array.isArray(ice) ||
            typeof iceValue !== 'string' ||
            utf8Length(iceValue) > MAX_ICE_CANDIDATE_BYTES
        ) {
            return null
        }
    }
    return candidate as MctierVoiceSignal
}

export class MctierVoiceSession {
    private readonly peers = new Map<string, PeerConnectionState>()
    private activePlayerIds = new Set<string>()
    private readonly mutedPeers = new Set<string>()
    private readonly peerVolumes = new Map<string, number>()
    private outputVolume = DEFAULT_OUTPUT_VOLUME
    private inputVolume = DEFAULT_INPUT_VOLUME
    private outputDeviceId: string | null = null
    private microphoneEnabled = true
    private closed = false
    private audioContext: AudioContext | null = null
    private inputGain: GainNode | null = null
    private outgoingStream: MediaStream
    private readonly rtcConfiguration: RTCConfiguration
    private readonly options: MctierVoiceSessionOptions

    constructor(options: MctierVoiceSessionOptions) {
        this.options = options
        this.outgoingStream = options.stream
        const iceServers = [
            ...(options.iceServer ? [{ urls: options.iceServer }] : []),
            { urls: 'stun:stun.qq.com:3478' },
            { urls: 'stun:stun.miwifi.com:3478' },
        ]
        this.rtcConfiguration = { iceServers }
        this.setupInputGain()
    }

    /** Routes the microphone through a gain node so the host can lower their
     * own outgoing level; peers receive the adjusted stream instead of the raw
     * capture. Environments without Web Audio keep the untouched capture. */
    private setupInputGain() {
        const AudioContextConstructor = globalThis.AudioContext
        if (!AudioContextConstructor) return
        try {
            const context = new AudioContextConstructor()
            const source = context.createMediaStreamSource(this.options.stream)
            const gain = context.createGain()
            gain.gain.value = this.inputVolume
            const destination = context.createMediaStreamDestination()
            source.connect(gain)
            gain.connect(destination)
            this.audioContext = context
            this.inputGain = gain
            this.outgoingStream = destination.stream
            void context.resume().catch(() => {})
        } catch {
            this.audioContext = null
            this.inputGain = null
            this.outgoingStream = this.options.stream
        }
    }

    async syncPeers(players: VoicePeer[]) {
        if (this.closed) return
        const next = new Set(
            players
                .map((player) => player.playerId)
                .filter((playerId) => playerId && playerId !== this.options.localPlayerId),
        )
        this.activePlayerIds = next

        for (const [playerId, peer] of this.peers) {
            if (!next.has(playerId)) this.closePeer(playerId, peer)
        }

        for (const playerId of next) {
            const peer = this.ensurePeer(playerId)
            if (this.options.localPlayerId.localeCompare(playerId) < 0) {
                await this.createOffer(playerId, peer)
            }
        }
    }

    async handleSignal(value: unknown) {
        if (this.closed) return
        const signal = parseMctierVoiceSignal(value, this.options.localPlayerId)
        if (!signal || !this.activePlayerIds.has(signal.from!)) return
        const peer = this.ensurePeer(signal.from!)
        if (signal.type === 'offer') {
            await peer.connection.setRemoteDescription(signal.offer!)
            peer.remoteDescriptionSet = true
            await this.flushCandidates(peer)
            const answer = await peer.connection.createAnswer()
            await peer.connection.setLocalDescription(answer)
            await this.options.send({
                type: 'answer',
                to: signal.from!,
                answer: peer.connection.localDescription?.toJSON() ?? answer,
            })
            return
        }
        if (signal.type === 'answer') {
            await peer.connection.setRemoteDescription(signal.answer!)
            peer.remoteDescriptionSet = true
            await this.flushCandidates(peer)
            return
        }
        if (!peer.remoteDescriptionSet) {
            if (peer.pendingCandidates.length < MAX_PENDING_CANDIDATES)
                peer.pendingCandidates.push(signal.candidate!)
            return
        }
        await peer.connection.addIceCandidate(signal.candidate!)
    }

    close() {
        if (this.closed) return
        this.closed = true
        for (const [playerId, peer] of this.peers) this.closePeer(playerId, peer)
        this.activePlayerIds.clear()
        this.peerVolumes.clear()
        this.options.stream.getTracks().forEach((track) => track.stop())
        if (this.audioContext) {
            void this.audioContext.close().catch(() => {})
            this.audioContext = null
            this.inputGain = null
        }
    }

    setMicrophoneEnabled(enabled: boolean) {
        if (this.closed || this.microphoneEnabled === enabled) return
        this.microphoneEnabled = enabled
        this.options.stream.getAudioTracks().forEach((track) => (track.enabled = enabled))
    }

    /** Applies a 0-1 gain to every remote player, including ones that join later. */
    setOutputVolume(volume: number) {
        this.outputVolume = clampVolume(volume)
        for (const [playerId, peer] of this.peers) this.applyPlaybackVolume(playerId, peer)
    }

    /** Applies a 0-1 gain to the local microphone capture, so everyone in the
     * room hears the host at the chosen level. */
    setInputVolume(volume: number) {
        this.inputVolume = clampVolume(volume)
        if (this.inputGain) this.inputGain.gain.value = this.inputVolume
    }

    /** Sets one remote player's level on top of the shared output volume. */
    setPeerVolume(playerId: string, volume: number) {
        this.peerVolumes.set(playerId, clampVolume(volume))
        const peer = this.peers.get(playerId)
        if (peer) this.applyPlaybackVolume(playerId, peer)
    }

    /** Mutes one remote player locally; the room is not told about it. */
    setPeerMuted(playerId: string, muted: boolean) {
        if (muted) this.mutedPeers.add(playerId)
        else this.mutedPeers.delete(playerId)
        const peer = this.peers.get(playerId)
        if (peer?.audio) peer.audio.muted = muted
    }

    private applyPlaybackVolume(playerId: string, peer: PeerConnectionState) {
        if (!peer.audio) return
        const peerVolume = this.peerVolumes.get(playerId) ?? DEFAULT_PEER_VOLUME
        peer.audio.volume = clampVolume(this.outputVolume * peerVolume)
        peer.audio.muted = this.mutedPeers.has(playerId)
    }

    /** Routes playback to a specific output device. Browsers without
     * `setSinkId` keep playing through the default device. Returns false when
     * the requested device could not be applied to at least one peer. */
    async setOutputDevice(deviceId: string | null): Promise<boolean> {
        this.outputDeviceId = deviceId
        let applied = true
        for (const peer of this.peers.values()) {
            if (!peer.audio) continue
            applied = (await this.applySink(peer.audio)) && applied
        }
        return applied
    }

    /** Applies the selected sink, falling back to the system default when the
     * device has disappeared; a removed headset must not leave playback bound
     * to a dead sink with no way back. */
    private async applySink(audio: HTMLAudioElement): Promise<boolean> {
        const sink = (audio as SinkCapableAudio).setSinkId
        if (!sink) return true
        const deviceId = this.outputDeviceId
        if (deviceId === null) {
            await sink.call(audio, '').catch(() => {})
            return true
        }
        try {
            await sink.call(audio, deviceId)
            return true
        } catch {
            this.outputDeviceId = null
            this.options.onOutputDeviceError?.()
            await sink.call(audio, '').catch(() => {})
            return false
        }
    }

    private ensurePeer(playerId: string): PeerConnectionState {
        const existing = this.peers.get(playerId)
        if (existing) return existing

        const connection = new RTCPeerConnection(this.rtcConfiguration)
        const peer: PeerConnectionState = {
            connection,
            remoteDescriptionSet: false,
            pendingCandidates: [],
            makingOffer: false,
            audio: null,
        }
        this.peers.set(playerId, peer)
        for (const track of this.outgoingStream.getAudioTracks())
            connection.addTrack(track, this.outgoingStream)

        connection.onicecandidate = (event) => {
            if (!event.candidate || this.closed || this.peers.get(playerId) !== peer) return
            void this.options
                .send({
                    type: 'ice-candidate',
                    to: playerId,
                    candidate: event.candidate.toJSON(),
                })
                .catch(() => {})
        }
        connection.ontrack = (event) => {
            if (this.closed || this.peers.get(playerId) !== peer) return
            if (!peer.audio) {
                const audio = document.createElement('audio')
                audio.autoplay = true
                audio.setAttribute('aria-hidden', 'true')
                audio.dataset.mctierVoicePeer = playerId
                if (this.outputDeviceId) void this.applySink(audio)
                peer.audio = audio
                this.applyPlaybackVolume(playerId, peer)
                document.body.append(audio)
            }
            peer.audio.srcObject = event.streams[0] ?? new MediaStream([event.track])
            this.options.onRemoteStream?.(playerId, peer.audio.srcObject)
            void peer.audio.play().catch(() => {})
        }
        connection.onconnectionstatechange = () =>
            this.options.onPeerState?.(playerId, connection.connectionState)
        connection.oniceconnectionstatechange = () =>
            this.options.onPeerState?.(playerId, connection.iceConnectionState)

        return peer
    }

    private async createOffer(playerId: string, peer: PeerConnectionState) {
        if (
            peer.makingOffer ||
            peer.connection.signalingState !== 'stable' ||
            peer.connection.localDescription
        )
            return
        peer.makingOffer = true
        try {
            const offer = await peer.connection.createOffer()
            await peer.connection.setLocalDescription(offer)
            await this.options.send({
                type: 'offer',
                to: playerId,
                offer: peer.connection.localDescription?.toJSON() ?? offer,
            })
        } finally {
            peer.makingOffer = false
        }
    }

    private async flushCandidates(peer: PeerConnectionState) {
        const pending = peer.pendingCandidates.splice(0)
        for (const candidate of pending) await peer.connection.addIceCandidate(candidate)
    }

    private closePeer(playerId: string, peer: PeerConnectionState) {
        const wasTracked = this.peers.delete(playerId)
        this.options.onRemoteStream?.(playerId, null)
        peer.connection.onicecandidate = null
        peer.connection.ontrack = null
        peer.connection.onconnectionstatechange = null
        peer.connection.oniceconnectionstatechange = null
        peer.connection.close()
        if (peer.audio) {
            peer.audio.pause()
            peer.audio.srcObject = null
            peer.audio.remove()
        }
        // The change handlers are detached before closing, so the owner would
        // otherwise never learn that this peer stopped existing.
        if (wasTracked) this.options.onPeerState?.(playerId, 'closed')
    }
}
