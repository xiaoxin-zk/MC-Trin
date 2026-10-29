<script setup lang="ts">
import { MicrophoneIcon, XIcon } from '@modrinth/assets'
import { Button, CopyCode, defineMessages, ProgressBar, StyledInput, useVIntl } from '@modrinth/ui'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

import { useMctierVoice } from '@/composables/useMctierVoice'
import {
    DEFAULT_MCTIER_NODE,
    DEFAULT_MCTIER_SIGNALING,
    type MctierInvite,
    type MctierState,
    parseMctierInviteCode,
} from '@/helpers/mctier'
import { multiplayer, multiplayerErrorText, validLocalPort } from '@/helpers/multiplayer'

const props = defineProps<{ state?: MctierState; initialInviteCode?: string }>()
const { formatMessage } = useVIntl()
const messages = defineMessages({
    title: { id: 'app.multiplayer.mctier.title', defaultMessage: 'MC-Trin LAN lobby' },
    description: {
        id: 'app.multiplayer.mctier.description',
        defaultMessage:
            'Create a public or private room, then invite friends with a shareable code.',
    },
    name: { id: 'app.multiplayer.mctier.name', defaultMessage: 'Lobby name' },
    player: { id: 'app.multiplayer.mctier.player', defaultMessage: 'Player name' },
    password: { id: 'app.multiplayer.mctier.password', defaultMessage: 'Lobby password' },
    port: { id: 'app.multiplayer.mctier.port', defaultMessage: 'Host Minecraft port' },
    node: { id: 'app.multiplayer.mctier.node', defaultMessage: 'EasyTier node' },
    signaling: { id: 'app.multiplayer.mctier.signaling', defaultMessage: 'Lobby service' },
    host: { id: 'app.multiplayer.mctier.host', defaultMessage: 'Create lobby' },
    join: { id: 'app.multiplayer.mctier.join', defaultMessage: 'Join lobby' },
    cancel: { id: 'app.multiplayer.mctier.cancel', defaultMessage: 'Cancel connection' },
    leave: { id: 'app.multiplayer.mctier.leave', defaultMessage: 'Leave lobby' },
    downloading: {
        id: 'app.multiplayer.mctier.downloading',
        defaultMessage: 'Preparing networking…',
    },
    connecting: {
        id: 'app.multiplayer.mctier.connecting',
        defaultMessage: 'Connecting to the lobby…',
    },
    stageDownloading: {
        id: 'app.multiplayer.mctier.stage.downloading',
        defaultMessage: 'Downloading the networking component…',
    },
    stageExtracting: {
        id: 'app.multiplayer.mctier.stage.extracting',
        defaultMessage: 'Extracting the networking component…',
    },
    stageVerifying: {
        id: 'app.multiplayer.mctier.stage.verifying',
        defaultMessage: 'Verifying the networking component…',
    },
    gamePortClosed: {
        id: 'app.multiplayer.mctier.host-hint-game-port',
        defaultMessage:
            'Minecraft is not listening on port {port} yet. Open the world to LAN in game so friends can join.',
    },
    hosting: { id: 'app.multiplayer.mctier.hosting', defaultMessage: 'Lobby created' },
    joined: {
        id: 'app.multiplayer.mctier.joined',
        defaultMessage: 'Minecraft connection verified',
    },
    address: {
        id: 'app.multiplayer.mctier.address',
        defaultMessage: 'Connect in Minecraft with this address:',
    },
    hostHint: {
        id: 'app.multiplayer.mctier.host-hint',
        defaultMessage:
            'Open your world to LAN first and enter the port shown by Minecraft. Guests use the invite code and enter their own LAN port.',
    },
    players: { id: 'app.multiplayer.mctier.players', defaultMessage: 'Players' },
    inviteCode: { id: 'app.multiplayer.mctier.invite-code', defaultMessage: 'Invite code' },
    joinByInvite: {
        id: 'app.multiplayer.mctier.join-by-invite',
        defaultMessage: 'Join using invite code',
    },
    visibility: {
        id: 'app.multiplayer.mctier.visibility',
        defaultMessage: 'Publish this room in the public lobby list',
    },
    publicWithPasswordHint: {
        id: 'app.multiplayer.mctier.public-with-password-hint',
        defaultMessage: 'Joining players must enter the room password or an invite code.',
    },
    privatePasswordHint: {
        id: 'app.multiplayer.mctier.private-password-hint',
        defaultMessage:
            'Private rooms stay out of the public lobby and require the room password to join.',
    },
    openPrivateHint: {
        id: 'app.multiplayer.mctier.open-private-hint',
        defaultMessage:
            'Private rooms stay out of the public lobby; without a password anyone who knows the room name can join.',
    },
    lobbyDescription: {
        id: 'app.multiplayer.mctier.lobby-description',
        defaultMessage: 'Room description',
    },
    maxPlayers: {
        id: 'app.multiplayer.mctier.max-players',
        defaultMessage: 'Player limit',
    },
    randomName: {
        id: 'app.multiplayer.mctier.random-name',
        defaultMessage: 'Random name',
    },
    randomPassword: {
        id: 'app.multiplayer.mctier.random-password',
        defaultMessage: 'Random password',
    },
    inviteHint: {
        id: 'app.multiplayer.mctier.invite-hint',
        defaultMessage:
            'The invite contains the room name, network node, and an encrypted password when needed.',
    },
    inviteInvalid: {
        id: 'app.multiplayer.mctier.invite-invalid',
        defaultMessage: 'Enter a valid MCTier invitation link or code.',
    },
    publicPending: {
        id: 'app.multiplayer.mctier.public-pending',
        defaultMessage: 'Updating public listing…',
    },
    inviteGenerating: {
        id: 'app.multiplayer.mctier.invite-generating',
        defaultMessage: 'Generating invite…',
    },
    generateInvite: {
        id: 'app.multiplayer.mctier.generate-invite',
        defaultMessage: 'Generate invite code',
    },
    voiceTitle: { id: 'app.multiplayer.mctier.voice.title', defaultMessage: 'Room voice' },
    voiceDescription: {
        id: 'app.multiplayer.mctier.voice.description',
        defaultMessage: 'Peer-to-peer voice connects after you allow microphone access.',
    },
    voiceStart: { id: 'app.multiplayer.mctier.voice.start', defaultMessage: 'Enable voice' },
    voiceMute: { id: 'app.multiplayer.mctier.voice.mute', defaultMessage: 'Mute microphone' },
    voiceUnmute: { id: 'app.multiplayer.mctier.voice.unmute', defaultMessage: 'Unmute microphone' },
    voiceStop: { id: 'app.multiplayer.mctier.voice.stop', defaultMessage: 'Leave voice' },
    voiceWaiting: {
        id: 'app.multiplayer.mctier.voice.waiting',
        defaultMessage: 'Waiting for another player to connect voice.',
    },
    voiceConnected: {
        id: 'app.multiplayer.mctier.voice.connected',
        defaultMessage:
            '{count, plural, one {Voice connected to # player} other {Voice connected to # players}}',
    },
    voiceErrorPermission: {
        id: 'app.multiplayer.mctier.voice.permission-error',
        defaultMessage:
            'Microphone permission was denied. Allow access in system settings and try again.',
    },
    voiceErrorUnsupported: {
        id: 'app.multiplayer.mctier.voice.unsupported-error',
        defaultMessage: 'This webview does not support microphone capture.',
    },
    voiceErrorUnavailable: {
        id: 'app.multiplayer.mctier.voice.unavailable-error',
        defaultMessage: 'Voice could not connect. Check the room network and microphone.',
    },
    voiceVolume: {
        id: 'app.multiplayer.mctier.voice.volume',
        defaultMessage: 'Output volume',
    },
    voiceInputVolume: {
        id: 'app.multiplayer.mctier.voice.input-volume',
        defaultMessage: 'Microphone volume',
    },
    voicePeerVolume: {
        id: 'app.multiplayer.mctier.voice.peer-volume',
        defaultMessage: 'Volume for {player}',
    },
    voiceMicrophone: {
        id: 'app.multiplayer.mctier.voice.microphone',
        defaultMessage: 'Microphone',
    },
    voiceSpeaker: {
        id: 'app.multiplayer.mctier.voice.speaker',
        defaultMessage: 'Speaker',
    },
    voiceSystemDefault: {
        id: 'app.multiplayer.mctier.voice.system-default',
        defaultMessage: 'System default',
    },
    voiceUnnamedDevice: {
        id: 'app.multiplayer.mctier.voice.unnamed-device',
        defaultMessage: 'Unnamed device',
    },
    voiceMutePlayer: {
        id: 'app.multiplayer.mctier.voice.mute-player',
        defaultMessage: 'Mute {player}',
    },
    voiceUnmutePlayer: {
        id: 'app.multiplayer.mctier.voice.unmute-player',
        defaultMessage: 'Unmute {player}',
    },
    voiceMicLevel: {
        id: 'app.multiplayer.mctier.voice.mic-level',
        defaultMessage: 'Mic level (speak to test)',
    },
    voicePushToTalk: {
        id: 'app.multiplayer.mctier.voice.push-to-talk',
        defaultMessage: 'Push-to-talk',
    },
    voicePushToTalkHint: {
        id: 'app.multiplayer.mctier.voice.push-to-talk-hint',
        defaultMessage: 'Hold F2 or the button to transmit; the microphone stays muted otherwise.',
    },
    voicePushToTalkHold: {
        id: 'app.multiplayer.mctier.voice.push-to-talk-hold',
        defaultMessage: 'Hold to talk',
    },
})

const lobbyName = ref('')
const playerName = ref('Player')
const password = ref('')
const port = ref('25565')
const serverNode = ref(DEFAULT_MCTIER_NODE)
const signalingServer = ref(DEFAULT_MCTIER_SIGNALING)
const inviteCodeInput = ref('')
const inviteCode = ref('')
const isPublic = ref(false)
const lobbyDescription = ref('')
const lobbyMaxPlayers = ref('')
const pending = ref(false)
const stopping = ref(false)
const visibilityPending = ref(false)
const inviteLoading = ref(false)
const error = ref('')
const busy = computed(
    () => pending.value || ['downloading', 'connecting'].includes(props.state?.status ?? ''),
)
const connected = computed(() => ['hosting', 'joined'].includes(props.state?.status ?? ''))
const valid = computed(
    () =>
        /^[\p{L}\p{N}_-]{4,32}$/u.test(lobbyName.value) &&
        playerName.value.trim().length > 0 &&
        validLocalPort(port.value) !== null,
)
const playerLimit = computed(() => {
    const parsed = Number.parseInt(lobbyMaxPlayers.value, 10)
    if (!Number.isFinite(parsed) || parsed < 2) return null
    return Math.min(parsed, 100)
})
const randomAlphabet = 'abcdefghjkmnpqrstuvwxyz23456789'
function randomCode(length: number) {
    const bytes = new Uint8Array(length)
    crypto.getRandomValues(bytes)
    return Array.from(bytes, (byte) => randomAlphabet[byte % randomAlphabet.length]).join('')
}
function generateLobbyName() {
    lobbyName.value = `mc-${randomCode(6)}`
}
function generatePassword() {
    password.value = randomCode(10)
}
const status = computed(() => {
    const value = props.state?.status
    return value && value in messages ? formatMessage(messages[value as 'hosting']) : ''
})
const preparing = computed(() => props.state?.status === 'downloading')
const preparationStage = computed(() => {
    switch (props.state?.download_stage) {
        case 'downloading':
            return formatMessage(messages.stageDownloading)
        case 'extracting':
            return formatMessage(messages.stageExtracting)
        case 'verifying':
            return formatMessage(messages.stageVerifying)
        default:
            return ''
    }
})
const hostHint = computed(() => {
    if (props.state?.host_hint !== 'game_port_closed') return ''
    return formatMessage(messages.gamePortClosed, {
        port: props.state?.game_port ?? 0,
    })
})
const voice = useMctierVoice(multiplayer.sendMctierVoiceSignal)
let unlistenVoice: UnlistenFn | undefined
let componentUnmounted = false

async function startVoice() {
    if (!props.state) return
    await voice.start(props.state)
    // Device labels are only readable after microphone permission is granted.
    if (voice.active.value) await voice.refreshDevices()
}

function onVolumeInput(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLInputElement)) return
    voice.setOutputVolume(Number(target.value) / 100)
}

function onInputVolumeInput(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLInputElement)) return
    voice.setInputVolume(Number(target.value) / 100)
}

function onPeerVolumeInput(event: Event, playerId: string) {
    const target = event.target
    if (!(target instanceof HTMLInputElement)) return
    voice.setPeerVolume(playerId, Number(target.value) / 100)
}

function onInputDeviceChange(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLSelectElement)) return
    void voice.setInputDevice(target.value || null)
}

function onOutputDeviceChange(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLSelectElement)) return
    void voice.setOutputDevice(target.value || null)
}

function onPttToggle(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLInputElement)) return
    voice.setPushToTalk(target.checked)
}

async function start(mode: 'create' | 'join', invite?: MctierInvite) {
    if (!valid.value || busy.value) return
    pending.value = true
    error.value = ''
    try {
        await multiplayer.startMctier({
            mode,
            lobby_name: invite?.lobby_name ?? lobbyName.value,
            password: invite?.password ?? password.value,
            player_name: playerName.value.trim(),
            server_node: invite?.server_node ?? serverNode.value.trim(),
            signaling_server: invite?.signaling_server ?? signalingServer.value.trim(),
            game_port: validLocalPort(port.value)!,
            is_public: mode === 'create' && isPublic.value,
            description:
                mode === 'create' && lobbyDescription.value.trim()
                    ? lobbyDescription.value.trim()
                    : null,
            max_players: mode === 'create' && playerLimit.value !== null ? playerLimit.value : null,
        })
    } catch (failure) {
        if (!stopping.value) error.value = multiplayerErrorText(failure)
    } finally {
        pending.value = false
    }
}

async function joinByInvite() {
    const invite = parseMctierInviteCode(inviteCodeInput.value)
    if (!invite) {
        error.value = formatMessage(messages.inviteInvalid)
        return
    }
    lobbyName.value = invite.lobby_name
    password.value = invite.password
    serverNode.value = invite.server_node
    signalingServer.value = invite.signaling_server
    await start('join', invite)
}

async function updateVisibility(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLInputElement)) return
    visibilityPending.value = true
    error.value = ''
    try {
        await multiplayer.setMctierPublic(target.checked)
    } catch (failure) {
        error.value = multiplayerErrorText(failure)
    } finally {
        visibilityPending.value = false
    }
}

async function refreshInvite() {
    if (inviteLoading.value) return
    inviteLoading.value = true
    try {
        inviteCode.value = await multiplayer.createMctierInvite()
    } catch (failure) {
        inviteCode.value = ''
        error.value = multiplayerErrorText(failure)
    } finally {
        inviteLoading.value = false
    }
}

// The state poll replaces the whole object, so the watcher compares a stable
// key: watching the pair as an array re-ran on every poll and minted a new
// invite each time, which made the code on screen keep changing.
const roomKey = computed(() => `${props.state?.status ?? ''}:${props.state?.mode ?? ''}`)

watch(
    roomKey,
    (key) => {
        if (key === 'hosting:create') {
            if (!inviteCode.value && !inviteLoading.value) void refreshInvite()
        } else {
            inviteCode.value = ''
        }
    },
    { immediate: true },
)

// The multiplayer state object is replaced on every backend poll. Voice only
// needs to resync when the room lifecycle or peer membership changes; watching
// the full object made it walk every peer on every 500ms state update.
const voiceRoomKey = computed(() => {
    const state = props.state
    if (!state) return ''
    const players = state.players
        .map((player) => player.playerId)
        .filter(Boolean)
        .sort()
        .join(',')
    return `${state.status}:${state.local_player_id ?? ''}:${players}`
})

watch(voiceRoomKey, () => voice.syncRoom(props.state), { immediate: true })

onMounted(() => {
    prefillInviteFromDeepLink()
    void listen<unknown>('mctier-voice-signal', (event) => {
        void voice.receiveSignal(event.payload)
    })
        .then((unlisten) => {
            if (componentUnmounted) unlisten()
            else unlistenVoice = unlisten
        })
        .catch((failure: unknown) => {
            console.warn('Could not subscribe to the room voice signaling event', failure)
        })
})

onUnmounted(() => {
    componentUnmounted = true
    unlistenVoice?.()
    voice.stop()
})

async function stop() {
    if (stopping.value) return
    stopping.value = true
    try {
        await multiplayer.stop()
    } catch (failure) {
        error.value = multiplayerErrorText(failure)
    } finally {
        stopping.value = false
    }
}

function prefillInviteFromDeepLink() {
    if (props.initialInviteCode?.trim()) inviteCodeInput.value = props.initialInviteCode.trim()
}
</script>

<template>
    <section class="flex flex-col gap-4 rounded-md bg-surface-1 p-5">
        <div>
            <h2 class="m-0 text-xl font-semibold">{{ formatMessage(messages.title) }}</h2>
            <p class="mb-0 text-sm text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.description) }}
            </p>
        </div>
        <p v-if="status" class="m-0 text-sm" role="status">{{ status }}</p>
        <ProgressBar
            v-if="preparing"
            full-width
            :progress="state?.download_progress ?? 0"
            :max="100"
            :waiting="state?.download_progress === null || state?.download_progress === undefined"
            :label="preparationStage"
            show-progress
        />
        <p v-if="hostHint" class="m-0 text-xs text-[var(--color-text-tertiary)]">{{ hostHint }}</p>
        <p
            v-if="error || state?.error_message"
            class="m-0 text-sm text-[var(--color-red)]"
            role="alert"
        >
            {{ error || state?.error_message }}
        </p>
        <template v-if="connected">
            <CopyCode v-if="state?.lobby_name" :text="state.lobby_name" />
            <template v-if="state?.mode === 'create'">
                <label class="flex items-center gap-2 text-sm">
                    <input
                        type="checkbox"
                        :checked="state.is_public"
                        :disabled="visibilityPending"
                        @change="updateVisibility"
                    />
                    <span>{{ formatMessage(messages.visibility) }}</span>
                    <span v-if="visibilityPending" role="status">
                        {{ formatMessage(messages.publicPending) }}
                    </span>
                </label>
                <p v-if="state.has_password" class="m-0 text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.publicWithPasswordHint) }}
                </p>
                <div v-if="inviteCode" class="flex min-w-0 flex-col gap-2">
                    <h3 class="m-0 text-base font-semibold">
                        {{ formatMessage(messages.inviteCode) }}
                    </h3>
                    <CopyCode :text="inviteCode" />
                    <p class="m-0 text-xs text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.inviteHint) }}
                    </p>
                </div>
                <p v-else-if="inviteLoading" class="m-0 text-sm" role="status">
                    {{ formatMessage(messages.inviteGenerating) }}
                </p>
                <Button v-else @click="refreshInvite">{{
                    formatMessage(messages.generateInvite)
                }}</Button>
            </template>
            <template v-if="state?.connect_address">
                <p class="m-0 text-sm">{{ formatMessage(messages.address) }}</p>
                <CopyCode :text="state.connect_address" />
            </template>
            <section
                class="flex min-w-0 flex-col gap-3 border-t border-solid border-surface-4 pt-4"
            >
                <div class="flex min-w-0 flex-wrap items-start justify-between gap-3">
                    <div class="min-w-0">
                        <h3 class="m-0 text-base font-semibold">
                            {{ formatMessage(messages.voiceTitle) }}
                        </h3>
                        <p class="mb-0 mt-1 text-sm text-[var(--color-text-tertiary)]">
                            {{ formatMessage(messages.voiceDescription) }}
                        </p>
                    </div>
                    <div class="flex shrink-0 flex-wrap gap-2">
                        <Button
                            v-if="!voice.active.value"
                            type="outlined"
                            :disabled="voice.starting.value"
                            @click="startVoice"
                        >
                            <MicrophoneIcon />
                            {{ formatMessage(messages.voiceStart) }}
                        </Button>
                        <template v-else>
                            <Button type="outlined" @click="voice.toggleMute">
                                <XIcon v-if="voice.muted.value" />
                                <MicrophoneIcon v-else />
                                {{
                                    formatMessage(
                                        voice.muted.value
                                            ? messages.voiceUnmute
                                            : messages.voiceMute,
                                    )
                                }}
                            </Button>
                            <Button type="quiet" @click="voice.stop">
                                <XIcon />
                                {{ formatMessage(messages.voiceStop) }}
                            </Button>
                        </template>
                    </div>
                </div>
                <p v-if="voice.active.value" class="m-0 text-sm" role="status">
                    {{
                        voice.connectedPeers.value
                            ? formatMessage(messages.voiceConnected, {
                                  count: voice.connectedPeers.value,
                              })
                            : formatMessage(messages.voiceWaiting)
                    }}
                </p>
                <div
                    v-if="voice.active.value"
                    class="flex min-w-0 flex-col gap-3 rounded-lg border border-solid border-surface-4 bg-surface-3 p-3"
                >
                    <label class="flex min-w-0 items-center gap-2 text-sm">
                        <span class="w-24 shrink-0">{{ formatMessage(messages.voiceVolume) }}</span>
                        <input
                            type="range"
                            min="0"
                            max="100"
                            step="5"
                            class="min-w-0 flex-1"
                            :value="Math.round(voice.outputVolume.value * 100)"
                            :aria-label="formatMessage(messages.voiceVolume)"
                            @input="onVolumeInput"
                        />
                        <span class="w-10 shrink-0 text-right tabular-nums">
                            {{ Math.round(voice.outputVolume.value * 100) }}%
                        </span>
                    </label>
                    <label class="flex min-w-0 items-center gap-2 text-sm">
                        <span class="w-24 shrink-0">{{
                            formatMessage(messages.voiceInputVolume)
                        }}</span>
                        <input
                            type="range"
                            min="0"
                            max="100"
                            step="5"
                            class="min-w-0 flex-1"
                            :value="Math.round(voice.inputVolume.value * 100)"
                            :aria-label="formatMessage(messages.voiceInputVolume)"
                            @input="onInputVolumeInput"
                        />
                        <span class="w-10 shrink-0 text-right tabular-nums">
                            {{ Math.round(voice.inputVolume.value * 100) }}%
                        </span>
                    </label>
                    <div class="flex min-w-0 items-center gap-2 text-sm">
                        <span class="w-24 shrink-0">{{
                            formatMessage(messages.voiceMicLevel)
                        }}</span>
                        <div
                            class="h-1.5 min-w-0 flex-1 overflow-hidden rounded bg-surface-5"
                            role="meter"
                            :aria-valuenow="voice.inputLevel.value"
                            aria-valuemin="0"
                            aria-valuemax="100"
                            :aria-label="formatMessage(messages.voiceMicLevel)"
                        >
                            <div
                                class="h-full bg-brand"
                                :style="{ width: `${voice.inputLevel.value}%` }"
                            />
                        </div>
                    </div>
                    <label class="flex min-w-0 items-center gap-2 text-sm">
                        <span class="w-24 shrink-0">{{
                            formatMessage(messages.voiceMicrophone)
                        }}</span>
                        <select
                            class="min-w-0 flex-1 rounded-md border border-solid border-surface-5 bg-surface-3 px-2 py-1"
                            :value="voice.inputDeviceId.value ?? ''"
                            :aria-label="formatMessage(messages.voiceMicrophone)"
                            @change="onInputDeviceChange"
                        >
                            <option value="">
                                {{ formatMessage(messages.voiceSystemDefault) }}
                            </option>
                            <option
                                v-for="device in voice.inputDevices.value"
                                :key="device.deviceId"
                                :value="device.deviceId"
                            >
                                {{ device.label || formatMessage(messages.voiceUnnamedDevice) }}
                            </option>
                        </select>
                    </label>
                    <label
                        v-if="voice.outputDevices.value.length"
                        class="flex min-w-0 items-center gap-2 text-sm"
                    >
                        <span class="w-24 shrink-0">{{
                            formatMessage(messages.voiceSpeaker)
                        }}</span>
                        <select
                            class="min-w-0 flex-1 rounded-md border border-solid border-surface-5 bg-surface-3 px-2 py-1"
                            :value="voice.outputDeviceId.value ?? ''"
                            :aria-label="formatMessage(messages.voiceSpeaker)"
                            @change="onOutputDeviceChange"
                        >
                            <option value="">
                                {{ formatMessage(messages.voiceSystemDefault) }}
                            </option>
                            <option
                                v-for="device in voice.outputDevices.value"
                                :key="device.deviceId"
                                :value="device.deviceId"
                            >
                                {{ device.label || formatMessage(messages.voiceUnnamedDevice) }}
                            </option>
                        </select>
                    </label>
                    <div
                        class="flex min-w-0 flex-col gap-2 border-t border-solid border-surface-4 pt-2"
                    >
                        <label class="flex min-w-0 items-center gap-2 text-sm">
                            <input
                                type="checkbox"
                                class="h-4 w-4"
                                :checked="voice.pushToTalk.value"
                                @change="onPttToggle"
                            />
                            <span>{{ formatMessage(messages.voicePushToTalk) }}</span>
                            <span class="text-xs font-normal text-[var(--color-text-tertiary)]">
                                F2
                            </span>
                        </label>
                        <p
                            v-if="voice.pushToTalk.value"
                            class="m-0 text-xs text-[var(--color-text-tertiary)]"
                        >
                            {{ formatMessage(messages.voicePushToTalkHint) }}
                        </p>
                        <button
                            v-if="voice.pushToTalk.value"
                            type="button"
                            class="min-w-0 cursor-pointer select-none rounded-md border border-solid border-brand px-3 py-2 text-sm font-semibold text-[var(--color-text-primary)]"
                            :class="voice.pttHeld.value ? 'bg-brand' : 'bg-surface-2'"
                            @pointerdown.prevent="voice.setPttHeld(true)"
                            @pointerup="voice.setPttHeld(false)"
                            @pointerleave="voice.setPttHeld(false)"
                            @pointercancel="voice.setPttHeld(false)"
                        >
                            {{ formatMessage(messages.voicePushToTalkHold) }}
                        </button>
                    </div>
                </div>
                <p v-if="voice.error.value" class="m-0 text-sm text-red" role="alert">
                    {{
                        formatMessage(
                            voice.error.value === 'permission'
                                ? messages.voiceErrorPermission
                                : voice.error.value === 'unsupported'
                                  ? messages.voiceErrorUnsupported
                                  : messages.voiceErrorUnavailable,
                        )
                    }}
                </p>
            </section>
            <h3 class="m-0 text-base font-semibold">{{ formatMessage(messages.players) }}</h3>
            <ul class="m-0 list-none p-0">
                <li
                    v-for="player in state?.players"
                    :key="player.playerId"
                    class="flex flex-wrap items-center justify-between gap-2 py-1 text-sm"
                >
                    <span class="flex min-w-0 items-center gap-2">
                        <span
                            v-if="
                                voice.active.value && voice.speakingPeers.value.has(player.playerId)
                            "
                            class="h-2 w-2 shrink-0 rounded-full bg-brand"
                            aria-hidden="true"
                        />
                        <span class="min-w-0 truncate">{{ player.playerName }}</span>
                    </span>
                    <span class="flex shrink-0 items-center gap-2">
                        <Button
                            v-if="voice.active.value && player.playerId !== state?.local_player_id"
                            type="quiet"
                            size="xs"
                            icon-only
                            :label="
                                formatMessage(
                                    voice.peerMuted.value[player.playerId]
                                        ? messages.voiceUnmutePlayer
                                        : messages.voiceMutePlayer,
                                    { player: player.playerName },
                                )
                            "
                            @click="voice.togglePeerMuted(player.playerId)"
                        >
                            <MicrophoneIcon v-if="!voice.peerMuted.value[player.playerId]" />
                            <XIcon v-else />
                        </Button>
                        <input
                            v-if="voice.active.value && player.playerId !== state?.local_player_id"
                            type="range"
                            min="0"
                            max="100"
                            step="5"
                            class="w-24"
                            :value="
                                Math.round((voice.peerVolumes.value[player.playerId] ?? 1) * 100)
                            "
                            :aria-label="
                                formatMessage(messages.voicePeerVolume, {
                                    player: player.playerName,
                                })
                            "
                            @input="onPeerVolumeInput($event, player.playerId)"
                        />
                        <span class="text-[var(--color-text-tertiary)]">{{
                            player.virtualIp
                        }}</span>
                    </span>
                </li>
            </ul>
            <Button :disabled="stopping" @click="stop">{{ formatMessage(messages.leave) }}</Button>
        </template>
        <template v-else-if="busy">
            <Button :disabled="stopping" @click="stop">{{ formatMessage(messages.cancel) }}</Button>
        </template>
        <form v-else class="grid gap-3 sm:grid-cols-2" @submit.prevent="start('create')">
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.name)
                }}<StyledInput v-model="lobbyName" name="lobby-name" :maxlength="32"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.player)
                }}<StyledInput v-model="playerName" name="player-name" :maxlength="64"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.password)
                }}<StyledInput
                    v-model="password"
                    name="lobby-password"
                    type="password"
                    :maxlength="128"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.port)
                }}<StyledInput v-model="port" name="game-port" inputmode="numeric"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.node)
                }}<StyledInput v-model="serverNode" name="server-node"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.signaling)
                }}<StyledInput v-model="signalingServer" name="signaling-server"
            /></label>
            <label class="flex flex-col gap-1 text-sm sm:col-span-2"
                >{{ formatMessage(messages.lobbyDescription)
                }}<StyledInput v-model="lobbyDescription" name="lobby-description" :maxlength="200"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.maxPlayers)
                }}<StyledInput
                    v-model="lobbyMaxPlayers"
                    name="max-players"
                    inputmode="numeric"
                    :maxlength="3"
                    placeholder="∞"
            /></label>
            <div class="flex flex-wrap items-center gap-2 sm:col-span-2">
                <Button type="quiet" @click="generateLobbyName">{{
                    formatMessage(messages.randomName)
                }}</Button>
                <Button type="quiet" @click="generatePassword">{{
                    formatMessage(messages.randomPassword)
                }}</Button>
            </div>
            <label class="flex items-center gap-2 text-sm sm:col-span-2">
                <input v-model="isPublic" name="public-lobby" type="checkbox" />
                <span>{{ formatMessage(messages.visibility) }}</span>
            </label>
            <p
                v-if="isPublic && password.trim()"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
            >
                {{ formatMessage(messages.publicWithPasswordHint) }}
            </p>
            <p
                v-else-if="!isPublic && password.trim()"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
            >
                {{ formatMessage(messages.privatePasswordHint) }}
            </p>
            <p
                v-else-if="!isPublic"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
            >
                {{ formatMessage(messages.openPrivateHint) }}
            </p>
            <p class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2">
                {{ formatMessage(messages.hostHint) }}
            </p>
            <div class="flex flex-wrap gap-2 sm:col-span-2">
                <Button :disabled="!valid" @click="start('create')">{{
                    formatMessage(messages.host)
                }}</Button>
                <Button :disabled="!valid" @click="start('join')">{{
                    formatMessage(messages.join)
                }}</Button>
            </div>
            <label class="flex min-w-0 flex-col gap-1 text-sm sm:col-span-2">
                {{ formatMessage(messages.inviteCode) }}
                <StyledInput
                    v-model="inviteCodeInput"
                    name="mctier-invite-code"
                    autocomplete="off"
                />
            </label>
            <div class="sm:col-span-2">
                <Button :disabled="busy || !inviteCodeInput.trim()" @click="joinByInvite">{{
                    formatMessage(messages.joinByInvite)
                }}</Button>
            </div>
        </form>
    </section>
</template>
