export type MctierVoiceSignal = {
    type: 'offer' | 'answer' | 'ice-candidate'
    from?: string
    to: string
    offer?: RTCSessionDescriptionInit
    answer?: RTCSessionDescriptionInit
    candidate?: RTCIceCandidateInit
}

/** Which noise-suppression implementation this webview can offer.
 *
 * `native` is the browser's own `noiseSuppression` constraint, which is always
 * available but is a plain filter rather than a learned model. `rnnoise` means
 * the bundled WebAssembly RNNoise worklet can run here. `unavailable` is neither,
 * so the UI hides the control instead of offering a switch that does nothing. */
export type NoiseSuppressionMode = 'native' | 'rnnoise' | 'unavailable'

/** Probes the two capabilities RNNoise needs, without downloading either asset.
 *
 * `audioWorklet` alone is not enough: an `AudioWorkletNode` also requires a
 * constructible `AudioContext`, and the processor is WebAssembly, which some
 * hardened builds disable via CSP. `WebAssembly.validate` is checked with a
 * trivial module so a present-but-broken implementation is caught here rather
 * than on the first audio frame. */
export function detectNoiseSuppressionMode(): NoiseSuppressionMode {
    if (typeof globalThis.AudioContext === 'undefined') return 'unavailable'
    if (typeof globalThis.AudioWorkletNode === 'undefined') return 'unavailable'
    if (typeof WebAssembly === 'undefined' || typeof WebAssembly.validate !== 'function') {
        return 'unavailable'
    }
    // The shortest valid module: the 8-byte wasm header alone. A build that
    // cannot validate this cannot run RNNoise either.
    const wasmMagic = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
    let wasmWorks = false
    try {
        wasmWorks = WebAssembly.validate(wasmMagic)
    } catch {
        wasmWorks = false
    }
    return wasmWorks ? 'rnnoise' : 'native'
}

/** The sample rate RNNoise assumes. Its `dist/index.d.ts` says the processor
 * "Assumes sample rate to be 48kHz", and its frame size (480 samples) is a wasm
 * constant meaning 10ms only at that rate. */
const RNNOISE_SAMPLE_RATE = 48000

/** The context plus whether it actually runs at RNNoise's rate.
 *
 * The processor never checks the context rate — its frame size comes from
 * `_rnnoise_get_frame_size()` in the wasm module, which is always 480, and its
 * only assertion is `frameSize !== 480`. So at 44.1kHz it still gets 480 samples,
 * which is ~10.88ms rather than 10ms: the denoiser runs, silently, about 8.8%
 * pitch-shifted. Nothing in the library detects this, which is why the rate has
 * to be confirmed here and the denoiser disabled when it does not match.
 *
 * Asking for a fixed rate is a preference, not a guarantee: some devices throw on
 * an unsupported `sampleRate` and others silently return the device default. A
 * rejected construction must never cost the user their call, so it is retried
 * without options — voice keeps working and only the denoiser is lost. */
type VoiceAudioContext = { context: AudioContext; atRnnoiseRate: boolean }

function createVoiceAudioContext(AudioContextConstructor: typeof AudioContext): VoiceAudioContext {
    let context: AudioContext
    try {
        context = new AudioContextConstructor({ sampleRate: RNNOISE_SAMPLE_RATE })
    } catch {
        // Unsupported rate: retry with the device default so the gain gate, the
        // level meter and the call itself all still work.
        context = new AudioContextConstructor()
    }
    return { context, atRnnoiseRate: context.sampleRate === RNNOISE_SAMPLE_RATE }
}

/** Loads RNNoise and returns an unconnected worklet node.
 *
 * Both assets are pulled in with `?url` so Vite emits them as separate files and
 * hands back their hashed URLs; neither is inlined because the wasm binary is far
 * above the 4096-byte `assetsInlineLimit`, and the worklet must stay a real
 * module URL for `addModule` to fetch it. The package exports both subpaths, so
 * no `vite.config.ts` change is needed: Vite copies imported package assets into
 * `dist/assets` like any other.
 *
 * `loadRnnoise` takes **both** the plain and the SIMD wasm URL and picks one
 * itself via a feature probe, so both must be shipped even though only one is
 * downloaded. Registration order matters: the processor has to be added to the
 * context before the node is constructed.
 *
 * Deferred to the first enable so a user who never turns noise reduction on
 * downloads none of it. */
async function createRnnoiseNode(context: AudioContext): Promise<AudioWorkletNode> {
    const [{ loadRnnoise, RnnoiseWorkletNode }, worklet, wasm, simdWasm] = await Promise.all([
        import('@sapphi-red/web-noise-suppressor'),
        import('@sapphi-red/web-noise-suppressor/rnnoiseWorklet.js?url'),
        import('@sapphi-red/web-noise-suppressor/rnnoise.wasm?url'),
        import('@sapphi-red/web-noise-suppressor/rnnoise_simd.wasm?url'),
    ])
    const wasmBinary = await loadRnnoise({
        url: wasm.default,
        simdUrl: simdWasm.default,
    })
    await context.audioWorklet.addModule(worklet.default)
    return new RnnoiseWorkletNode(context, { wasmBinary, maxChannels: 1 })
}

/** Releases the wasm heap held by a worklet node.
 *
 * `destroy()` only exists on the library's node class, and a node the caller
 * already dropped may not have it, so the call is guarded rather than assumed. */
function destroyRnnoiseNode(node: AudioWorkletNode) {
    const destroy = (node as AudioWorkletNode & { destroy?: () => void }).destroy
    if (typeof destroy !== 'function') return
    try {
        destroy.call(node)
    } catch {
        // The processor may already have shut down with its context.
    }
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
    /** Swaps the RNNoise loader, for tests only.
     *
     * The real loader needs Vite to resolve its `?url` imports, which a plain
     * `node --test` process cannot do, so without this seam the post-load
     * generation guard is unreachable in tests and any assertion about it would
     * pass for the wrong reason. Production never sets it. */
    loadNoiseNode?: (context: AudioContext) => Promise<AudioWorkletNode>
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
export function clampVolume(volume: number): number {
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
    private voiceActivationEnabled = false
    private voiceActivity = true
    private closed = false
    private audioContext: AudioContext | null = null
    private inputGain: GainNode | null = null
    /** Kept so the graph can be rewired around the gain node when RNNoise is
     * toggled at runtime. */
    private inputSource: MediaStreamAudioSourceNode | null = null
    private inputDestination: MediaStreamAudioDestinationNode | null = null
    /** Inserted between the capture and the gain node when RNNoise is on. */
    private noiseNode: AudioWorkletNode | null = null
    private noiseSuppressionEnabled = false
    /** Whether the graph runs at the rate RNNoise assumes. False means the
     * denoiser must stay off: it cannot detect a rate mismatch itself, so
     * enabling it there would pitch-shift the user's voice unnoticed. */
    private audioContextAtRnnoiseRate = false
    /** Bumped by every enable/disable/close. A load takes hundreds of
     * milliseconds, so an enable can still be in flight when the user flips the
     * switch back off; without this counter the late arrival would connect the
     * node and leave the UI reading "off" while audio was being processed. */
    private noiseGeneration = 0
    private outgoingStream: MediaStream
    private readonly rtcConfiguration: RTCConfiguration
    private readonly options: MctierVoiceSessionOptions
    /** Label of the capture device in use, so the owner can tell a virtual
     * microphone (which already denoises) from a physical one. */
    readonly activeCaptureLabel: string
    /** A GainNode gate needs Web Audio; without it the call is plain open-mic. */
    readonly supportsVoiceActivation: boolean
    /** `setSinkId` is the only way to choose a speaker. */
    readonly supportsOutputDevice: boolean

    constructor(options: MctierVoiceSessionOptions) {
        this.options = options
        this.outgoingStream = options.stream
        this.activeCaptureLabel = options.stream.getAudioTracks()[0]?.label ?? ''
        this.supportsVoiceActivation = Boolean(globalThis.AudioContext)
        this.supportsOutputDevice =
            typeof HTMLAudioElement !== 'undefined' && 'setSinkId' in HTMLAudioElement.prototype
        const iceServers = [
            ...(options.iceServer ? [{ urls: options.iceServer }] : []),
            { urls: 'stun:stun.qq.com:3478' },
            { urls: 'stun:stun.miwifi.com:3478' },
        ]
        this.rtcConfiguration = { iceServers }
        this.setupInputGain()
    }

    /** Builds `source → [rnnoise] → gain → destination`.
     *
     * The gain node both scales the outgoing level and gates voice activation,
     * so RNNoise has to sit *before* it: noise removed after the gate would keep
     * the gate's tail audible, because the gate closes on the raw signal while
     * the residue it just unmuted is still in flight.
     */
    private setupInputGain() {
        const AudioContextConstructor = globalThis.AudioContext
        if (!AudioContextConstructor) return
        try {
            const { context, atRnnoiseRate } = createVoiceAudioContext(AudioContextConstructor)
            const source = context.createMediaStreamSource(this.options.stream)
            const gain = context.createGain()
            const destination = context.createMediaStreamDestination()
            source.connect(gain)
            gain.connect(destination)
            this.audioContext = context
            this.audioContextAtRnnoiseRate = atRnnoiseRate
            this.inputGain = gain
            this.inputSource = source
            this.inputDestination = destination
            this.outgoingStream = destination.stream
            void context.resume().catch(() => {})
            // Start from the real state rather than the nominal volume: a
            // session that begins muted must not open at full gain.
            this.applyInputGain()
        } catch {
            this.audioContext = null
            this.audioContextAtRnnoiseRate = false
            this.inputGain = null
            this.inputSource = null
            this.inputDestination = null
            this.outgoingStream = this.options.stream
        }
    }

    /** Turns RNNoise on or off by rewiring the graph around the gain node.
     *
     * Loading is deferred to the first enable: the worklet module plus whichever
     * of the two wasm builds the SIMD probe selects come to roughly 217 KB, and a
     * user who never asks for noise reduction should not download any of it.
     * Failures are swallowed down to a warning so a missing asset degrades to the
     * native path instead of breaking the call.
     *
     * Returns whether RNNoise is actually running afterwards. */
    async setNoiseSuppressionEnabled(enabled: boolean): Promise<boolean> {
        if (this.closed) return false
        // Any state change invalidates an in-flight load, including this one.
        const generation = (this.noiseGeneration += 1)
        if (!enabled) {
            this.teardownNoiseSuppression()
            return false
        }
        if (!this.audioContext || !this.inputSource || !this.inputGain || !this.inputDestination) {
            return false
        }
        // RNNoise assumes 48kHz and cannot tell that it is wrong: its frame size
        // is a wasm constant, so at any other rate it would process 480 samples
        // as if they were 10ms and pitch-shift the voice. Refuse rather than run
        // it, and let the caller fall back to the browser's own filter.
        if (!this.audioContextAtRnnoiseRate) return false
        if (this.noiseNode) {
            this.noiseSuppressionEnabled = true
            return true
        }
        try {
            const node = await (this.options.loadNoiseNode ?? createRnnoiseNode)(this.audioContext)
            // The user may have turned the switch off, or the whole session may
            // have closed, while the worklet and wasm were downloading. The
            // request that arrives last must not win: drop this node untouched
            // rather than wiring it into a graph that is already the wrong shape.
            if (this.closed || generation !== this.noiseGeneration) {
                node.disconnect()
                destroyRnnoiseNode(node)
                return false
            }
            // Rewire only after the node exists, so a failed load leaves the
            // untouched source → gain path in place.
            this.inputSource.disconnect()
            this.inputSource.connect(node)
            node.connect(this.inputGain)
            this.noiseNode = node
            this.noiseSuppressionEnabled = true
            return true
        } catch (failure) {
            console.warn('RNNoise noise suppression could not start', failure)
            return false
        }
    }

    get noiseSuppressionActive(): boolean {
        return this.noiseSuppressionEnabled && this.noiseNode !== null
    }

    /** Exposed so the owner can show the native fallback instead of a dead
     * switch when this machine's audio graph does not run at 48kHz. */
    get supportsNoiseSuppression(): boolean {
        return this.audioContextAtRnnoiseRate
    }

    /** Unhooks the worklet and drops the reference so its thread and buffers can
     * be reclaimed; with the node gone there is no per-frame RNNoise work left.
     *
     * `RnnoiseWorkletNode.destroy()` is what actually frees the wasm heap — a
     * plain `disconnect()` only unplugs it from the graph. */
    private teardownNoiseSuppression() {
        this.noiseSuppressionEnabled = false
        if (!this.noiseNode) return
        const node = this.noiseNode
        this.noiseNode = null
        try {
            node.disconnect()
        } catch {
            // The node may already be detached; nothing left to release.
        }
        destroyRnnoiseNode(node)
        if (this.inputSource && this.inputGain) {
            try {
                this.inputSource.disconnect()
            } catch {
                // Already disconnected by the wiring above.
            }
            this.inputSource.connect(this.inputGain)
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
        // Invalidate any enable still waiting on the worklet download, so it
        // cannot connect a node into a context that is about to be closed.
        this.noiseGeneration += 1
        for (const [playerId, peer] of this.peers) this.closePeer(playerId, peer)
        this.activePlayerIds.clear()
        this.peerVolumes.clear()
        this.teardownNoiseSuppression()
        this.options.stream.getTracks().forEach((track) => track.stop())
        if (this.audioContext) {
            void this.audioContext.close().catch(() => {})
            this.audioContext = null
            this.inputGain = null
            this.inputSource = null
            this.inputDestination = null
        }
    }

    /** Mutes the outgoing signal. With Web Audio the capture track stays live and
     * the gain node is what closes: disabling the track would also starve the
     * local analyser, so voice activation could never re-open it and the level
     * meter would read silence while muted. Without Web Audio there is no gate,
     * so the track itself is the only available switch. */
    setMicrophoneEnabled(enabled: boolean) {
        if (this.closed || this.microphoneEnabled === enabled) return
        this.microphoneEnabled = enabled
        if (this.inputGain) {
            this.applyInputGain()
            return
        }
        this.options.stream.getAudioTracks().forEach((track) => (track.enabled = enabled))
    }

    setVoiceActivationEnabled(enabled: boolean) {
        if (this.closed || !this.supportsVoiceActivation) return
        if (this.voiceActivationEnabled === enabled) return
        this.voiceActivationEnabled = enabled
        if (!enabled) this.voiceActivity = true
        this.applyInputGain()
    }

    setVoiceActivity(speaking: boolean) {
        if (this.closed || this.voiceActivity === speaking) return
        this.voiceActivity = speaking
        this.applyInputGain()
    }

    private applyInputGain() {
        if (this.inputGain) {
            this.inputGain.gain.value =
                this.microphoneEnabled && (!this.voiceActivationEnabled || this.voiceActivity)
                    ? this.inputVolume
                    : 0
        }
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
        this.applyInputGain()
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
