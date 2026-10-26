import { computed, ref } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import {
    MctierVoiceSession,
    type MctierVoiceSignal,
    parseMctierVoiceSignal,
    type VoicePeer,
} from '../helpers/mctier-voice'

type VoiceError = 'permission' | 'unsupported' | 'unavailable'
type QueuedSignal = { signal: unknown; size: number }

const MAX_PENDING_SIGNALS = 128
const MAX_PENDING_SIGNAL_BYTES = 1024 * 1024
const MAX_SIGNAL_BYTES = 256 * 1024

export type VoiceDevice = { deviceId: string; label: string }

type VoicePreferences = {
    muted: boolean
    outputVolume: number
    inputDeviceId: string | null
    outputDeviceId: string | null
}

const PREFERENCES_KEY = 'mctier.voice.preferences'
const DEFAULT_PREFERENCES: VoicePreferences = {
    muted: false,
    outputVolume: 1,
    inputDeviceId: null,
    outputDeviceId: null,
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
            outputVolume: clampVolume(Number(parsed.outputVolume ?? 1)),
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
    const outputVolume = ref(preferences.outputVolume)
    const inputDeviceId = ref<string | null>(preferences.inputDeviceId)
    const outputDeviceId = ref<string | null>(preferences.outputDeviceId)
    const inputDevices = ref<VoiceDevice[]>([])
    const outputDevices = ref<VoiceDevice[]>([])
    const peerMuted = ref<Record<string, boolean>>({})
    const error = ref<VoiceError | null>(null)
    const peerStates = ref<Record<string, string>>({})
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
    const pendingSignals: QueuedSignal[] = []
    let pendingSignalBytes = 0

    function persistPreferences() {
        storePreferences({
            muted: muted.value,
            outputVolume: outputVolume.value,
            inputDeviceId: inputDeviceId.value,
            outputDeviceId: outputDeviceId.value,
        })
    }

    /** Opens the saved microphone, falling back to the system default once when
     * that device is gone (unplugged headset, driver reset). */
    async function openMicrophone(): Promise<MediaStream> {
        const constraints: MediaTrackConstraints = {
            autoGainControl: true,
            echoCancellation: true,
            noiseSuppression: true,
        }
        const deviceId = inputDeviceId.value
        if (!deviceId) {
            return navigator.mediaDevices.getUserMedia({ audio: constraints, video: false })
        }
        try {
            return await navigator.mediaDevices.getUserMedia({
                audio: { ...constraints, deviceId: { exact: deviceId } },
                video: false,
            })
        } catch (cause) {
            if (cause instanceof DOMException && cause.name === 'NotAllowedError') throw cause
            inputDeviceId.value = null
            persistPreferences()
            return navigator.mediaDevices.getUserMedia({ audio: constraints, video: false })
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
        if (size > MAX_SIGNAL_BYTES) return

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
        signalDelivery = delivery.catch(() => {
            if (session === target) error.value = 'unavailable'
        })
        return signalDelivery
    }

    async function start(state: MctierState) {
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
            stream = await openMicrophone()
            if (generation !== startGeneration) {
                stream.getTracks().forEach((track) => track.stop())
                return
            }

            startingSession = new MctierVoiceSession({
                localPlayerId,
                stream,
                iceServer: state.voice_ice_server,
                send,
                onPeerState: (playerId, nextState) => {
                    peerStates.value = { ...peerStates.value, [playerId]: nextState }
                },
            })
            session = startingSession
            sessionReady = false
            startingSession.setOutputVolume(outputVolume.value)
            if (outputDeviceId.value) void startingSession.setOutputDevice(outputDeviceId.value)
            if (muted.value) startingSession.setMicrophoneEnabled(false)
            for (const [playerId, isMuted] of Object.entries(peerMuted.value)) {
                if (isMuted) startingSession.setPeerMuted(playerId, true)
            }
            await startingSession.syncPeers(state.players as VoicePeer[])
            if (generation !== startGeneration || session !== startingSession) {
                startingSession.close()
                return
            }

            active.value = true
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
            error.value =
                cause instanceof DOMException && cause.name === 'NotAllowedError'
                    ? 'permission'
                    : 'unavailable'
        } finally {
            if (generation === startGeneration) starting.value = false
        }
    }

    async function syncPeers(players: VoicePeer[]) {
        if (session) await session.syncPeers(players).catch(() => (error.value = 'unavailable'))
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
        session?.setMicrophoneEnabled(!muted.value)
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
        clearPendingSignals()
        signalDelivery = Promise.resolve()
    }

    function setOutputVolume(value: number) {
        outputVolume.value = clampVolume(value)
        persistPreferences()
        session?.setOutputVolume(outputVolume.value)
    }

    function togglePeerMuted(playerId: string) {
        const next = !peerMuted.value[playerId]
        peerMuted.value = { ...peerMuted.value, [playerId]: next }
        session?.setPeerMuted(playerId, next)
    }

    async function setInputDevice(deviceId: string | null) {
        inputDeviceId.value = deviceId
        persistPreferences()
        const state = lastState
        if (!active.value || !state) return
        // Microphones are attached when peer connections are built, so switching
        // input rebuilds the session; the room and its signalling are untouched.
        stop()
        await start(state)
    }

    async function setOutputDevice(deviceId: string | null) {
        outputDeviceId.value = deviceId
        persistPreferences()
        await session?.setOutputDevice(deviceId)
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

    function syncRoom(state: MctierState | undefined) {
        if (!state || state.status === 'idle' || state.status === 'error') {
            roomLocalPlayerId = null
            lastState = null
            stop()
            return
        }
        lastState = state
        const nextLocalPlayerId = state.local_player_id
        if (roomLocalPlayerId && nextLocalPlayerId && roomLocalPlayerId !== nextLocalPlayerId) {
            stop()
        }
        roomLocalPlayerId = nextLocalPlayerId
        if (session) void syncPeers(state.players as VoicePeer[])
    }

    return {
        active,
        starting,
        muted,
        error,
        connectedPeers,
        outputVolume,
        inputDeviceId,
        outputDeviceId,
        inputDevices,
        outputDevices,
        peerMuted,
        start,
        syncRoom,
        receiveSignal,
        toggleMute,
        togglePeerMuted,
        setOutputVolume,
        setInputDevice,
        setOutputDevice,
        refreshDevices,
        stop,
    }
}
