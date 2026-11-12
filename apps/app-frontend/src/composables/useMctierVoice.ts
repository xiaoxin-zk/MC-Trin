import { computed, onScopeDispose, ref } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import {
    MAX_SIGNAL_FRAME_BYTES,
    MctierVoiceSession,
    type MctierVoiceSignal,
    parseMctierVoiceSignal,
    type VoicePeer,
} from '../helpers/mctier-voice'
import { createVoiceActivityMonitor, type VoiceActivityMonitor } from '../helpers/voice-activity'

type VoiceError =
    'permission' | 'unsupported' | 'unavailable' | 'input-device-lost' | 'output-device-lost'
type QueuedSignal = { signal: unknown; size: number }

const MAX_PENDING_SIGNALS = 128
// The queue only bridges the window before the session is ready, so it is held
// to one frame budget: a larger backlog could not be replayed before the peers
// time out, and holding megabytes of candidates for a session that never opens
// is worse than dropping the oldest ones.
const MAX_PENDING_SIGNAL_BYTES = MAX_SIGNAL_FRAME_BYTES

export type VoiceDevice = { deviceId: string; label: string }

type VoicePreferences = {
    muted: boolean
    voiceActivation: boolean
    outputVolume: number
    inputVolume: number
    peerVolumes: Record<string, number>
    inputDeviceId: string | null
    outputDeviceId: string | null
}

const PREFERENCES_KEY = 'mctier.voice.preferences'
const DEFAULT_PREFERENCES: VoicePreferences = {
    muted: false,
    voiceActivation: false,
    outputVolume: 1,
    inputVolume: 1,
    peerVolumes: {},
    inputDeviceId: null,
    outputDeviceId: null,
}

/** Virtual microphones already apply their own noise and echo handling;
 * asking Chromium for the same processing again doubles it, which clips the
 * first syllable and makes the level pump. */
const PREPROCESSED_INPUT_PATTERNS = [
    /nvidia broadcast/i,
    /rtx voice/i,
    /amd noise suppression/i,
    /krisp/i,
    /voicemeeter/i,
    /vb-audio/i,
    /virtual audio/i,
]

function looksPreprocessed(label: string) {
    return PREPROCESSED_INPUT_PATTERNS.some((pattern) => pattern.test(label))
}

function clampPeerVolumes(value: unknown): Record<string, number> {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    const entries = Object.entries(value as Record<string, unknown>)
        .filter(([playerId, volume]) => playerId && typeof volume === 'number')
        .map(([playerId, volume]) => [playerId, clampVolume(Number(volume))] as const)
    return Object.fromEntries(entries)
}

function clampVolume(volume: number): number {
    if (!Number.isFinite(volume)) return 1
    return Math.min(1, Math.max(0, volume))
}

/** Device ids and the chosen mic state outlive the room, so they are kept in
 * localStorage rather than in memory only. Storage being unavailable (private
 * mode, tests) just means the defaults are used for the session. */
function loadPreferences(): VoicePreferences {
    try {
        if (typeof localStorage === 'undefined') return { ...DEFAULT_PREFERENCES }
        const raw = localStorage.getItem(PREFERENCES_KEY)
        if (!raw) return { ...DEFAULT_PREFERENCES }
        const parsed = JSON.parse(raw) as Partial<VoicePreferences>
        return {
            muted: parsed.muted === true,
            voiceActivation: parsed.voiceActivation === true,
            outputVolume: clampVolume(Number(parsed.outputVolume ?? 1)),
            inputVolume: clampVolume(Number(parsed.inputVolume ?? 1)),
            peerVolumes: clampPeerVolumes(parsed.peerVolumes),
            inputDeviceId:
                typeof parsed.inputDeviceId === 'string' && parsed.inputDeviceId
                    ? parsed.inputDeviceId
                    : null,
            outputDeviceId:
                typeof parsed.outputDeviceId === 'string' && parsed.outputDeviceId
                    ? parsed.outputDeviceId
                    : null,
        }
    } catch {
        return { ...DEFAULT_PREFERENCES }
    }
}

function storePreferences(preferences: VoicePreferences) {
    try {
        if (typeof localStorage === 'undefined') return
        localStorage.setItem(PREFERENCES_KEY, JSON.stringify(preferences))
    } catch {
        // Persisting the preference is best effort.
    }
}

export function useMctierVoice(send: (signal: MctierVoiceSignal) => Promise<void>) {
    const preferences = loadPreferences()
    const active = ref(false)
    const starting = ref(false)
    const muted = ref(preferences.muted)
    const voiceActivation = ref(preferences.voiceActivation)
    const outputVolume = ref(preferences.outputVolume)
    const inputVolume = ref(preferences.inputVolume)
    const peerVolumes = ref<Record<string, number>>(preferences.peerVolumes)
    const inputDeviceId = ref<string | null>(preferences.inputDeviceId)
    const outputDeviceId = ref<string | null>(preferences.outputDeviceId)
    const inputDevices = ref<VoiceDevice[]>([])
    const outputDevices = ref<VoiceDevice[]>([])
    /** Capabilities this webview actually advertises, so the panel never offers a
     * control that would silently do nothing. A GainNode is the only way to gate
     * the outgoing signal without touching the track, and `setSinkId` is the
     * only way to route playback, so both are feature-detected rather than
     * assumed. */
    const inputVolumeSupported = ref(Boolean(globalThis.AudioContext))
    const voiceActivationSupported = ref(Boolean(globalThis.AudioContext))
    const outputDeviceSupported = ref(
        typeof HTMLAudioElement !== 'undefined' && 'setSinkId' in HTMLAudioElement.prototype,
    )
    const peerMuted = ref<Record<string, boolean>>({})
    const error = ref<VoiceError | null>(null)
    const peerStates = ref<Record<string, string>>({})
    const speakingPeers = ref<Set<string>>(new Set())
    const inputLevel = ref(0)
    const connectedPeers = computed(
        () =>
            Object.values(peerStates.value).filter(
                (state) => state === 'connected' || state === 'completed',
            ).length,
    )
    let session: MctierVoiceSession | null = null
    let sessionReady = false
    let roomLocalPlayerId: string | null = null
    let lastState: MctierState | null = null
    let startGeneration = 0
    let signalDelivery = Promise.resolve()
    let monitor: VoiceActivityMonitor | null = null
    let captureStream: MediaStream | null = null
    const pendingSignals: QueuedSignal[] = []
    let pendingSignalBytes = 0

    function startMonitoring(localPlayerId: string) {
        monitor?.stop()
        monitor = createVoiceActivityMonitor({
            onSpeaking: (id, speaking) => {
                const next = new Set(speakingPeers.value)
                if (speaking) next.add(id)
                else next.delete(id)
                speakingPeers.value = next
            },
            onLevel: (percent) => {
                inputLevel.value = percent
            },
            onLocalSpeaking: (speaking) => {
                session?.setVoiceActivity(speaking)
            },
        })
        monitor.watch(localPlayerId, captureStream, true)
    }

    function stopMonitoring() {
        monitor?.stop()
        monitor = null
        captureStream = null
        speakingPeers.value = new Set()
        inputLevel.value = 0
    }

    /** The microphone stays open by default. Voice activation gates the outgoing
     * gain while the raw capture remains observable by the analyser. */
    function applyMicrophoneState() {
        session?.setMicrophoneEnabled(!muted.value)
        session?.setVoiceActivationEnabled(!muted.value && voiceActivation.value)
    }

    function persistPreferences() {
        storePreferences({
            muted: muted.value,
            voiceActivation: voiceActivation.value,
            outputVolume: outputVolume.value,
            inputVolume: inputVolume.value,
            peerVolumes: peerVolumes.value,
            inputDeviceId: inputDeviceId.value,
            outputDeviceId: outputDeviceId.value,
        })
    }

    async function deviceIsGone(kind: MediaDeviceKind, deviceId: string): Promise<boolean> {
        try {
            const devices = await navigator.mediaDevices.enumerateDevices()
            const known = devices.filter((device) => device.kind === kind && device.deviceId)
            return known.length > 0 && !known.some((device) => device.deviceId === deviceId)
        } catch {
            return false
        }
    }

    async function usesPreprocessedInput(deviceId: string | null): Promise<boolean> {
        try {
            const devices = await navigator.mediaDevices.enumerateDevices()
            const inputs = devices.filter((device) => device.kind === 'audioinput')
            const selected = deviceId
                ? inputs.find((device) => device.deviceId === deviceId)
                : (inputs.find((device) => device.deviceId === 'default' || !device.deviceId) ??
                  inputs[0])
            return selected ? looksPreprocessed(selected.label) : false
        } catch {
            return false
        }
    }

    async function requestMicrophone(deviceId: string | null): Promise<MediaStream> {
        const processed = await usesPreprocessedInput(deviceId)
        const supported = navigator.mediaDevices.getSupportedConstraints?.() ?? {}
        const constraints: MediaTrackConstraints = {}
        if ('autoGainControl' in supported) constraints.autoGainControl = !processed
        if ('echoCancellation' in supported) constraints.echoCancellation = !processed
        if ('noiseSuppression' in supported) {
            constraints.noiseSuppression = !processed
        }
        const stream = await navigator.mediaDevices.getUserMedia({
            audio: deviceId ? { ...constraints, deviceId: { exact: deviceId } } : constraints,
            video: false,
        })
        // Labels are empty until microphone access has been granted once, so
        // the first capture may only reveal a virtual device after opening it.
        const track = stream.getAudioTracks()[0]
        if (!processed && track && looksPreprocessed(track.label)) {
            const processingConstraints: MediaTrackConstraints = {}
            const supported = navigator.mediaDevices.getSupportedConstraints?.() ?? {}
            if ('autoGainControl' in supported) processingConstraints.autoGainControl = false
            if ('echoCancellation' in supported) processingConstraints.echoCancellation = false
            if ('noiseSuppression' in supported) processingConstraints.noiseSuppression = false
            await track.applyConstraints(processingConstraints).catch(() => {})
        }
        return stream
    }

    /** Opens the saved microphone, falling back to the system default once when
     * that device is gone (unplugged headset, driver reset). A device that is
     * only busy stays selected so the next session retries it. */
    async function openMicrophone(deviceId: string | null, allowFallback: boolean) {
        try {
            return await requestMicrophone(deviceId)
        } catch (cause) {
            if (!allowFallback || !deviceId) throw cause
            if (cause instanceof DOMException && cause.name === 'NotAllowedError') throw cause
            if ((await deviceIsGone('audioinput', deviceId)) && inputDeviceId.value === deviceId) {
                inputDeviceId.value = null
                persistPreferences()
            }
            return requestMicrophone(null)
        }
    }

    function enqueueSignal(signal: unknown) {
        let size: number
        try {
            const serialized = JSON.stringify(signal)
            if (!serialized) return
            size = new TextEncoder().encode(serialized).length
        } catch {
            return
        }
        if (size > MAX_SIGNAL_FRAME_BYTES) return

        while (
            pendingSignals.length >= MAX_PENDING_SIGNALS ||
            pendingSignalBytes + size > MAX_PENDING_SIGNAL_BYTES
        ) {
            const removed = pendingSignals.shift()
            if (!removed) break
            pendingSignalBytes -= removed.size
        }
        pendingSignals.push({ signal, size })
        pendingSignalBytes += size
    }

    function clearPendingSignals() {
        pendingSignals.length = 0
        pendingSignalBytes = 0
    }

    function deliverSignal(target: MctierVoiceSession, signal: MctierVoiceSignal) {
        const delivery = signalDelivery.then(async () => {
            if (session !== target || !sessionReady) return
            await target.handleSignal(signal)
        })
        // One rejected offer or a late ICE candidate only concerns that peer.
        // Latching a global error here reported a healthy call as broken, and
        // the peer's own connection state already carries the failure.
        signalDelivery = delivery.catch((failure: unknown) => {
            console.warn('MCTier voice signalling failed for one peer', failure)
        })
        return signalDelivery
    }

    async function start(state: MctierState) {
        await startSession(state)
    }

    async function startSession(state: MctierState, preparedStream?: MediaStream) {
        if (active.value || starting.value) return
        error.value = null
        const localPlayerId = state.local_player_id
        if (!localPlayerId || !navigator.mediaDevices?.getUserMedia) {
            error.value = 'unsupported'
            return
        }
        roomLocalPlayerId = localPlayerId
        const generation = ++startGeneration
        signalDelivery = Promise.resolve()
        starting.value = true
        let stream: MediaStream | null = null
        let startingSession: MctierVoiceSession | null = null
        try {
            stream = preparedStream ?? (await openMicrophone(inputDeviceId.value, true))
            if (generation !== startGeneration) {
                stream.getTracks().forEach((track) => track.stop())
                return
            }
            prunePeerPreferences(state.players as VoicePeer[])

            startingSession = new MctierVoiceSession({
                localPlayerId,
                stream,
                iceServer: state.voice_ice_server,
                send,
                onPeerState: (playerId, nextState) => {
                    // A session that has already been replaced must not edit the
                    // state of the one now serving the call.
                    if (session !== startingSession) return
                    // A torn-down peer has to leave the map entirely, otherwise
                    // its last state keeps it counted as connected.
                    peerStates.value =
                        nextState === 'closed'
                            ? Object.fromEntries(
                                  Object.entries(peerStates.value).filter(
                                      ([id]) => id !== playerId,
                                  ),
                              )
                            : { ...peerStates.value, [playerId]: nextState }
                },
                onRemoteStream: (playerId, remoteStream) => {
                    monitor?.watch(playerId, remoteStream)
                },
                onOutputDeviceError: () => {
                    if (session !== startingSession) return
                    if (outputDeviceId.value !== null) {
                        outputDeviceId.value = null
                        persistPreferences()
                    }
                    error.value = 'output-device-lost'
                    void refreshDevices()
                },
            })
            session = startingSession
            sessionReady = false
            // Capability flags come from the session's own feature detection:
            // routing through Web Audio says nothing about whether the browser
            // can denoise, so no processing capability is claimed here.
            voiceActivationSupported.value = startingSession.supportsVoiceActivation
            outputDeviceSupported.value = startingSession.supportsOutputDevice
            startingSession.setOutputVolume(outputVolume.value)
            startingSession.setInputVolume(inputVolume.value)
            if (outputDeviceId.value) void startingSession.setOutputDevice(outputDeviceId.value)
            applyMicrophoneState()
            for (const [playerId, isMuted] of Object.entries(peerMuted.value)) {
                if (isMuted) startingSession.setPeerMuted(playerId, true)
            }
            for (const [playerId, volume] of Object.entries(peerVolumes.value)) {
                startingSession.setPeerVolume(playerId, volume)
            }
            // The first negotiation is part of starting the call, so a failure
            // here still means voice did not come up.
            await startingSession.syncPeers(state.players as VoicePeer[])
            if (generation !== startGeneration || session !== startingSession) {
                startingSession.close()
                return
            }

            active.value = true
            captureStream = stream
            watchCaptureTrack(stream)
            startMonitoring(localPlayerId)
            const queued = pendingSignals.splice(0)
            pendingSignalBytes = 0
            sessionReady = true
            for (const entry of queued) {
                const signal = parseMctierVoiceSignal(entry.signal, localPlayerId)
                if (signal) void deliverSignal(startingSession, signal)
            }
            await signalDelivery
        } catch (cause) {
            if (generation !== startGeneration) {
                stream?.getTracks().forEach((track) => track.stop())
                return
            }
            startingSession?.close()
            if (session === startingSession) session = null
            sessionReady = false
            clearPendingSignals()
            stream?.getTracks().forEach((track) => track.stop())
            active.value = false
            stopMonitoring()
            error.value =
                cause instanceof DOMException && cause.name === 'NotAllowedError'
                    ? 'permission'
                    : 'unavailable'
        } finally {
            if (generation === startGeneration) starting.value = false
        }
    }

    /** A capture track ends when its device is unplugged or its permission is
     * revoked. The call cannot continue one-way, so stop it and say why. */
    function watchCaptureTrack(stream: MediaStream) {
        for (const track of stream.getAudioTracks()) {
            track.onended = () => handleCaptureLost(stream)
        }
    }

    function handleCaptureLost(stream: MediaStream) {
        if (captureStream !== stream || !active.value) return
        stop()
        error.value = 'input-device-lost'
        void refreshDevices()
    }

    function activeCaptureDeviceId(): string | null {
        const track = captureStream?.getAudioTracks()[0]
        if (!track) return null
        try {
            return track.getSettings().deviceId ?? null
        } catch {
            return null
        }
    }

    async function syncPeers(players: VoicePeer[]) {
        // Per-peer offers can fail while the rest of the call stays healthy;
        // only the affected peer connection reports a problem.
        if (session) await session.syncPeers(players).catch(() => {})
    }

    async function receiveSignal(signal: unknown) {
        if (!session || !sessionReady) {
            enqueueSignal(signal)
            return
        }
        const localPlayerId = roomLocalPlayerId
        if (!localPlayerId) return
        const parsed = parseMctierVoiceSignal(signal, localPlayerId)
        if (parsed) await deliverSignal(session, parsed)
    }

    function toggleMute() {
        muted.value = !muted.value
        persistPreferences()
        applyMicrophoneState()
    }

    function stop() {
        startGeneration += 1
        session?.close()
        session = null
        sessionReady = false
        starting.value = false
        active.value = false
        peerStates.value = {}
        error.value = null
        stopMonitoring()
        clearPendingSignals()
        signalDelivery = Promise.resolve()
    }

    function setVoiceActivation(enabled: boolean) {
        if (enabled && !voiceActivationSupported.value) return
        voiceActivation.value = enabled
        persistPreferences()
        applyMicrophoneState()
    }

    function setOutputVolume(value: number) {
        outputVolume.value = clampVolume(value)
        persistPreferences()
        session?.setOutputVolume(outputVolume.value)
    }

    function setInputVolume(value: number) {
        if (!inputVolumeSupported.value) return
        inputVolume.value = clampVolume(value)
        persistPreferences()
        session?.setInputVolume(inputVolume.value)
    }

    /** Player ids are minted per registration, so anything keyed by them has to
     * be dropped when the roster moves on or the stored blob grows forever. */
    function prunePeerPreferences(players: VoicePeer[]) {
        const present = new Set(
            players
                .map((player) => player.playerId)
                .filter((playerId) => playerId && playerId !== roomLocalPlayerId),
        )
        const volumes = Object.fromEntries(
            Object.entries(peerVolumes.value).filter(([playerId]) => present.has(playerId)),
        )
        if (Object.keys(volumes).length !== Object.keys(peerVolumes.value).length) {
            peerVolumes.value = volumes
            persistPreferences()
        }
        const mutedPeers = Object.fromEntries(
            Object.entries(peerMuted.value).filter(([playerId]) => present.has(playerId)),
        )
        if (Object.keys(mutedPeers).length !== Object.keys(peerMuted.value).length) {
            peerMuted.value = mutedPeers
        }
        const states = Object.fromEntries(
            Object.entries(peerStates.value).filter(([playerId]) => present.has(playerId)),
        )
        if (Object.keys(states).length !== Object.keys(peerStates.value).length) {
            peerStates.value = states
        }
    }

    function setPeerVolume(playerId: string, value: number) {
        peerVolumes.value = { ...peerVolumes.value, [playerId]: clampVolume(value) }
        persistPreferences()
        session?.setPeerVolume(playerId, peerVolumes.value[playerId])
    }

    function togglePeerMuted(playerId: string) {
        const next = !peerMuted.value[playerId]
        peerMuted.value = { ...peerMuted.value, [playerId]: next }
        session?.setPeerMuted(playerId, next)
    }

    async function setInputDevice(deviceId: string | null) {
        const previousDeviceId = inputDeviceId.value
        if (deviceId === previousDeviceId) return
        const state = lastState
        if (!active.value || !state) {
            inputDeviceId.value = deviceId
            persistPreferences()
            return
        }
        error.value = null
        // Rebuilding the session tears the working call down, so prove the new
        // device opens before committing to it.
        let stream: MediaStream
        try {
            stream = await openMicrophone(deviceId, false)
        } catch {
            // The call is still healthy on the previous device and the select
            // snaps back, so this is a failed choice rather than a session
            // failure; a global error banner would misreport the live call.
            await refreshDevices()
            return
        }
        const current = lastState
        if (!active.value || !current || current.local_player_id !== state.local_player_id) {
            stream.getTracks().forEach((track) => track.stop())
            return
        }
        inputDeviceId.value = deviceId
        persistPreferences()
        stop()
        await startSession(current, stream)
        if (active.value) return
        // The new device opened but the rebuilt session still failed, so put the
        // previous preference back and bring the call up on the device that
        // worked instead of leaving voice off.
        inputDeviceId.value = previousDeviceId
        persistPreferences()
        const restored = await openMicrophone(previousDeviceId, true).catch(() => null)
        if (!restored) {
            error.value = 'unavailable'
            return
        }
        error.value = null
        await startSession(current, restored)
    }

    async function setOutputDevice(deviceId: string | null) {
        if (!outputDeviceSupported.value) return
        outputDeviceId.value = deviceId
        persistPreferences()
        const applied = await session?.setOutputDevice(deviceId)
        if (applied === false) {
            // The session already fell back to the default sink and cleared its
            // own choice; keep the stored preference honest as well.
            outputDeviceId.value = null
            persistPreferences()
        } else if (error.value === 'output-device-lost') {
            error.value = null
        }
    }

    async function refreshDevices() {
        const mediaDevices = navigator.mediaDevices
        if (!mediaDevices?.enumerateDevices) return
        const devices = await mediaDevices.enumerateDevices().catch(() => [])
        inputDevices.value = devices
            .filter((device) => device.kind === 'audioinput')
            .map((device) => ({ deviceId: device.deviceId, label: device.label }))
        outputDevices.value = devices
            .filter((device) => device.kind === 'audiooutput')
            .map((device) => ({ deviceId: device.deviceId, label: device.label }))
    }

    /** A device that disappears mid-call never reaches the room state, so the
     * device list has to be driven by the media layer instead. Not every
     * removal fires `ended` on the capture track, so the running capture is
     * checked here as well. */
    async function onDeviceChange() {
        await refreshDevices()
        const capture = captureStream
        const captureDeviceId = activeCaptureDeviceId()
        if (capture && captureDeviceId && (await deviceIsGone('audioinput', captureDeviceId))) {
            handleCaptureLost(capture)
        }
        if (inputDeviceId.value && (await deviceIsGone('audioinput', inputDeviceId.value))) {
            inputDeviceId.value = null
            persistPreferences()
        }
        if (outputDeviceId.value && (await deviceIsGone('audiooutput', outputDeviceId.value))) {
            outputDeviceId.value = null
            persistPreferences()
            await session?.setOutputDevice(null)
        }
    }

    const mediaDevices = typeof navigator === 'undefined' ? undefined : navigator.mediaDevices
    if (mediaDevices && typeof mediaDevices.addEventListener === 'function') {
        mediaDevices.addEventListener('devicechange', onDeviceChange)
    }

    onScopeDispose(() => {
        if (mediaDevices && typeof mediaDevices.removeEventListener === 'function') {
            mediaDevices.removeEventListener('devicechange', onDeviceChange)
        }
        stopMonitoring()
    })

    function syncRoom(state: MctierState | undefined) {
        if (!state || state.status === 'idle' || state.status === 'error') {
            roomLocalPlayerId = null
            lastState = null
            stop()
            return
        }
        // The panel is visible before a call starts, so a failure left over from
        // the previous room would otherwise be the first thing a new room shows.
        // Only a real status transition clears it: a live device failure keeps
        // its status, and its message has to stay readable.
        if (!active.value && !starting.value && lastState?.status !== state.status) {
            error.value = null
        }
        lastState = state
        const nextLocalPlayerId = state.local_player_id
        if (roomLocalPlayerId && nextLocalPlayerId && roomLocalPlayerId !== nextLocalPlayerId) {
            stop()
        }
        roomLocalPlayerId = nextLocalPlayerId
        prunePeerPreferences(state.players as VoicePeer[])
        if (session) void syncPeers(state.players as VoicePeer[])
    }

    return {
        active,
        starting,
        muted,
        error,
        connectedPeers,
        outputVolume,
        inputVolume,
        peerVolumes,
        inputDeviceId,
        outputDeviceId,
        inputDevices,
        outputDevices,
        peerMuted,
        voiceActivation,
        voiceActivationSupported,
        inputVolumeSupported,
        outputDeviceSupported,
        speakingPeers,
        inputLevel,
        start,
        syncRoom,
        receiveSignal,
        toggleMute,
        togglePeerMuted,
        setOutputVolume,
        setInputVolume,
        setPeerVolume,
        setInputDevice,
        setOutputDevice,
        setVoiceActivation,
        refreshDevices,
        stop,
    }
}
