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
}

const MAX_SIGNAL_BYTES = 256 * 1024
const MAX_PENDING_CANDIDATES = 128
const DEFAULT_OUTPUT_VOLUME = 1

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
        if (JSON.stringify(value).length > MAX_SIGNAL_BYTES) return null
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
            sdp.length > MAX_SIGNAL_BYTES
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
            iceValue.length > 16 * 1024
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
    private outputVolume = DEFAULT_OUTPUT_VOLUME
    private outputDeviceId: string | null = null
    private closed = false
    private readonly rtcConfiguration: RTCConfiguration
    private readonly options: MctierVoiceSessionOptions

    constructor(options: MctierVoiceSessionOptions) {
        this.options = options
        const iceServers = [
            ...(options.iceServer ? [{ urls: options.iceServer }] : []),
            { urls: 'stun:stun.qq.com:3478' },
            { urls: 'stun:stun.miwifi.com:3478' },
        ]
        this.rtcConfiguration = { iceServers }
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
        this.options.stream.getTracks().forEach((track) => track.stop())
    }

    setMicrophoneEnabled(enabled: boolean) {
        if (this.closed) return
        this.options.stream.getAudioTracks().forEach((track) => (track.enabled = enabled))
    }

    /** Applies a 0-1 gain to every remote player, including ones that join later. */
    setOutputVolume(volume: number) {
        this.outputVolume = clampVolume(volume)
        for (const peer of this.peers.values()) {
            if (peer.audio) peer.audio.volume = this.outputVolume
        }
    }

    /** Mutes one remote player locally; the room is not told about it. */
    setPeerMuted(playerId: string, muted: boolean) {
        if (muted) this.mutedPeers.add(playerId)
        else this.mutedPeers.delete(playerId)
        const peer = this.peers.get(playerId)
        if (peer?.audio) peer.audio.muted = muted
    }

    /** Routes playback to a specific output device. Browsers without
     * `setSinkId` keep playing through the default device. */
    async setOutputDevice(deviceId: string | null) {
        this.outputDeviceId = deviceId
        for (const peer of this.peers.values()) {
            if (peer.audio) await this.applySink(peer.audio).catch(() => {})
        }
    }

    private async applySink(audio: HTMLAudioElement) {
        const sink = (audio as SinkCapableAudio).setSinkId
        if (!sink) return
        await sink.call(audio, this.outputDeviceId ?? '')
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
        for (const track of this.options.stream.getAudioTracks())
            connection.addTrack(track, this.options.stream)

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
                audio.volume = this.outputVolume
                audio.muted = this.mutedPeers.has(playerId)
                if (this.outputDeviceId) void this.applySink(audio)
                peer.audio = audio
                document.body.append(audio)
            }
            peer.audio.srcObject = event.streams[0] ?? new MediaStream([event.track])
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
        this.peers.delete(playerId)
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
    }
}
