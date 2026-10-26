import { computed, ref } from 'vue'

import {
    MctierVoiceSession,
    parseMctierVoiceSignal,
    type MctierVoiceSignal,
    type VoicePeer,
} from '../helpers/mctier-voice'
import type { MctierState } from '@/helpers/mctier'

type VoiceError = 'permission' | 'unsupported' | 'unavailable'
type QueuedSignal = { signal: unknown; size: number }

const MAX_PENDING_SIGNALS = 128
const MAX_PENDING_SIGNAL_BYTES = 1024 * 1024
const MAX_SIGNAL_BYTES = 256 * 1024

export function useMctierVoice(send: (signal: MctierVoiceSignal) => Promise<void>) {
    const active = ref(false)
    const starting = ref(false)
    const muted = ref(false)
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
    let startGeneration = 0
    let signalDelivery = Promise.resolve()
    const pendingSignals: QueuedSignal[] = []
    let pendingSignalBytes = 0

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
            stream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    autoGainControl: true,
                    echoCancellation: true,
                    noiseSuppression: true,
                },
                video: false,
            })
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
        if (!session) return
        muted.value = !muted.value
        session.setMicrophoneEnabled(!muted.value)
    }

    function stop() {
        startGeneration += 1
        session?.close()
        session = null
        sessionReady = false
        starting.value = false
        active.value = false
        muted.value = false
        peerStates.value = {}
        error.value = null
        clearPendingSignals()
        signalDelivery = Promise.resolve()
    }

    function syncRoom(state: MctierState | undefined) {
        if (!state || state.status === 'idle' || state.status === 'error') {
            roomLocalPlayerId = null
            stop()
            return
        }
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
        start,
        syncRoom,
        receiveSignal,
        toggleMute,
        stop,
    }
}
