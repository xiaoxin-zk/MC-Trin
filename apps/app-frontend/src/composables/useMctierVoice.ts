import { computed, onScopeDispose, ref, watch } from 'vue'

import type { MctierState } from '@/helpers/mctier'

import {
    MctierVoiceSession,
    type MctierVoiceSignal,
    parseMctierVoiceSignal,
    type VoicePeer,
} from '../helpers/mctier-voice'
import { createVoiceActivityMonitor, type VoiceActivityMonitor } from '../helpers/voice-activity'

type VoiceError = 'permission' | 'unsupported' | 'unavailable'
type QueuedSignal = { signal: unknown; size: number }

const MAX_PENDING_SIGNALS = 128
const MAX_PENDING_SIGNAL_BYTES = 1024 * 1024
const MAX_SIGNAL_BYTES = 256 * 1024

export type VoiceDevice = { deviceId: string; label: string }

type VoicePreferences = {
    muted: boolean
    outputVolume: number
    inputVolume: number
    peerVolumes: Record<string, number>
    inputDeviceId: string | null
    outputDeviceId: string | null
    pushToTalk: boolean
    pushToTalkKey: string
}

const PREFERENCES_KEY = 'mctier.voice.preferences'
export const DEFAULT_PUSH_TO_TALK_KEY = 'F2'
const DEFAULT_PREFERENCES: VoicePreferences = {
    muted: false,
    outputVolume: 1,
    inputVolume: 1,
    peerVolumes: {},
    inputDeviceId: null,
    outputDeviceId: null,
    pushToTalk: false,
    pushToTalkKey: DEFAULT_PUSH_TO_TALK_KEY,
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
            pushToTalk: parsed.pushToTalk === true,
            pushToTalkKey:
                typeof parsed.pushToTalkKey === 'string' && parsed.pushToTalkKey
                    ? parsed.pushToTalkKey
                    : DEFAULT_PUSH_TO_TALK_KEY,
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
    const inputVolume = ref(preferences.inputVolume)
    const peerVolumes = ref<Record<string, number>>(preferences.peerVolumes)
    const inputDeviceId = ref<string | null>(preferences.inputDeviceId)
    const outputDeviceId = ref<string | null>(preferences.outputDeviceId)
    const inputDevices = ref<VoiceDevice[]>([])
    const outputDevices = ref<VoiceDevice[]>([])
    const peerMuted = ref<Record<string, boolean>>({})
    const error = ref<VoiceError | null>(null)
    const peerStates = ref<Record<string, string>>({})
    const pushToTalk = ref(preferences.pushToTalk)
    const pushToTalkKey = ref(preferences.pushToTalkKey)
    const pttHeld = ref(false)
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

    /** The microphone transmits unless the master mute is on, and in
     * push-to-talk mode only while the key or button is held. */
    function applyMicrophoneState() {
        session?.setMicrophoneEnabled(!muted.value && (!pushToTalk.value || pttHeld.value))
    }

    function persistPreferences() {
        storePreferences({
            muted: muted.value,
            outputVolume: outputVolume.value,
            inputVolume: inputVolume.value,
            peerVolumes: peerVolumes.value,
            inputDeviceId: inputDeviceId.value,
            outputDeviceId: outputDeviceId.value,
            pushToTalk: pushToTalk.value,
            pushToTalkKey: pushToTalkKey.value,
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
                onRemoteStream: (playerId, remoteStream) => {
                    monitor?.watch(playerId, remoteStream)
                },
            })
            session = startingSession
            sessionReady = false
            startingSession.setOutputVolume(outputVolume.value)
            startingSession.setInputVolume(inputVolume.value)
            if (outputDeviceId.value) void startingSession.setOutputDevice(outputDeviceId.value)
            startingSession.setMicrophoneEnabled(
                !muted.value && (!pushToTalk.value || pttHeld.value),
            )
            for (const [playerId, isMuted] of Object.entries(peerMuted.value)) {
                if (isMuted) startingSession.setPeerMuted(playerId, true)
            }
            for (const [playerId, volume] of Object.entries(peerVolumes.value)) {
                startingSession.setPeerVolume(playerId, volume)
            }
            await startingSession.syncPeers(state.players as VoicePeer[])
            if (generation !== startGeneration || session !== startingSession) {
                startingSession.close()
                return
            }

            active.value = true
            captureStream = stream
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
        pttHeld.value = false
        stopMonitoring()
        clearPendingSignals()
        signalDelivery = Promise.resolve()
    }

    function setOutputVolume(value: number) {
        outputVolume.value = clampVolume(value)
        persistPreferences()
        session?.setOutputVolume(outputVolume.value)
    }

    function setInputVolume(value: number) {
        inputVolume.value = clampVolume(value)
        persistPreferences()
        session?.setInputVolume(inputVolume.value)
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

    /** Push-to-talk listens for the configured key while a voice session is
     * active, so the hotkey costs nothing when voice is off. */
    function pttTargetIsEditable(target: EventTarget | null) {
        if (!(target instanceof HTMLElement)) return false
        return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
    }
    function onPttKeyDown(event: KeyboardEvent) {
        if (event.code !== pushToTalkKey.value || event.repeat || pttTargetIsEditable(event.target))
            return
        event.preventDefault()
        pttHeld.value = true
    }
    function onPttKeyUp(event: KeyboardEvent) {
        if (event.code !== pushToTalkKey.value) return
        event.preventDefault()
        pttHeld.value = false
    }
    function onWindowBlur() {
        pttHeld.value = false
    }
    function removePttListeners() {
        window.removeEventListener('keydown', onPttKeyDown)
        window.removeEventListener('keyup', onPttKeyUp)
        window.removeEventListener('blur', onWindowBlur)
    }

    function setPushToTalk(enabled: boolean) {
        pushToTalk.value = enabled
        if (!enabled) pttHeld.value = false
        persistPreferences()
    }

    /** Rebinds push-to-talk; the listener reads the ref, so no re-registration
     * is needed when the key changes mid-session. */
    function setPushToTalkKey(code: string) {
        if (!code) return
        pushToTalkKey.value = code
        pttHeld.value = false
        persistPreferences()
    }

    function setPttHeld(held: boolean) {
        if (!pushToTalk.value || pttHeld.value === held) return
        pttHeld.value = held
    }

    watch(
        [active, pushToTalk],
        ([isActive, enabled]) => {
            if (isActive && enabled) {
                window.addEventListener('keydown', onPttKeyDown)
                window.addEventListener('keyup', onPttKeyUp)
                window.addEventListener('blur', onWindowBlur)
            } else {
                removePttListeners()
                pttHeld.value = false
            }
        },
        { immediate: true },
    )
    onScopeDispose(() => {
        removePttListeners()
        stopMonitoring()
    })

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
        inputVolume,
        peerVolumes,
        inputDeviceId,
        outputDeviceId,
        inputDevices,
        outputDevices,
        peerMuted,
        pushToTalk,
        pushToTalkKey,
        pttHeld,
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
        setPushToTalk,
        setPushToTalkKey,
        setPttHeld,
        refreshDevices,
        stop,
    }
}
