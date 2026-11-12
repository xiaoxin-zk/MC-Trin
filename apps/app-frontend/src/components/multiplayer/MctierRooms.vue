<script setup lang="ts">
import { GlobeIcon, LockIcon, MicrophoneIcon, XIcon } from '@modrinth/assets'
import {
    Button,
    Checkbox,
    CopyCode,
    defineMessages,
    ProgressBar,
    StyledInput,
    useVIntl,
} from '@modrinth/ui'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

import SettingsRow from '@/components/ui/settings/SettingsRow.vue'
import { useMctierVoice } from '@/composables/useMctierVoice'
import {
    DEFAULT_MCTIER_NODE,
    DEFAULT_MCTIER_SIGNALING,
    isValidDirectServerNode,
    isValidMctierLobbyName,
    isValidMctierPlayerName,
    isValidMctierSignalingServer,
    type MctierInvite,
    type MctierState,
    parseMctierInviteCode,
} from '@/helpers/mctier'
import {
    multiplayer,
    multiplayerErrorText,
    sanitizeErrorText,
    validLocalPort,
} from '@/helpers/multiplayer'

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
    nodeInvalid: {
        id: 'app.multiplayer.mctier.node-invalid',
        defaultMessage: 'Enter a node address such as tcp://node.example:11010.',
    },
    nameMissing: {
        id: 'app.multiplayer.mctier.name-missing',
        defaultMessage: 'Enter a lobby name so friends can find the room:',
    },
    nameInvalid: {
        id: 'app.multiplayer.mctier.name-invalid',
        defaultMessage: 'Lobby names use 4-32 letters, numbers, underscores, or hyphens.',
    },
    playerMissing: {
        id: 'app.multiplayer.mctier.player-missing',
        defaultMessage: 'Enter the player name friends will see.',
    },
    playerInvalid: {
        id: 'app.multiplayer.mctier.player-invalid',
        defaultMessage:
            'Player names must be 1-64 characters and cannot contain control characters.',
    },
    portInvalid: {
        id: 'app.multiplayer.mctier.port-invalid',
        defaultMessage: 'Minecraft port must be between 1 and 65535.',
    },
    signalingInvalid: {
        id: 'app.multiplayer.mctier.signaling-invalid',
        defaultMessage:
            'Lobby service must use wss://, or ws:// only on this machine (localhost, 127.0.0.1, [::1]).',
    },
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
    visibilityTitle: {
        id: 'app.multiplayer.mctier.visibility-title',
        defaultMessage: 'Who can find this room',
    },
    visibilityPrivateTitle: {
        id: 'app.multiplayer.mctier.visibility-private-title',
        defaultMessage: 'Private room',
    },
    visibilityPrivateDescription: {
        id: 'app.multiplayer.mctier.visibility-private-description',
        defaultMessage: 'Only people you send the invite code to can join.',
    },
    visibilityPublicTitle: {
        id: 'app.multiplayer.mctier.visibility-public-title',
        defaultMessage: 'List in the public lobby',
    },
    visibilityPublicDescription: {
        id: 'app.multiplayer.mctier.visibility-public-description',
        defaultMessage:
            'Listed in the lobby list for anyone to join. A listed room cannot have a password.',
    },
    publicNoPassword: {
        id: 'app.multiplayer.mctier.public-no-password',
        defaultMessage:
            'A listed room cannot have a password. To keep a password, choose a private room and share an invite code.',
    },
    publishPublished: {
        id: 'app.multiplayer.mctier.publish-published',
        defaultMessage: 'Listed publicly',
    },
    publishUnconfirmed: {
        id: 'app.multiplayer.mctier.publish-unconfirmed',
        defaultMessage:
            'Sent to the lobby service. It did not confirm the listing, so give it a moment to appear.',
    },
    publishPrivate: {
        id: 'app.multiplayer.mctier.publish-private',
        defaultMessage: 'Private, not listed',
    },
    publicWithPasswordHint: {
        id: 'app.multiplayer.mctier.public-with-password-hint',
        defaultMessage:
            'This room is listed for anyone to join. Unlisting it keeps the room private.',
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
    publicOpenHint: {
        id: 'app.multiplayer.mctier.public-open-hint',
        defaultMessage:
            'Listed publicly with no password: anyone browsing the lobby list can join directly.',
    },
    publishNotListed: {
        id: 'app.multiplayer.mctier.publish-not-listed',
        defaultMessage:
            'The lobby service accepted the request but does not list this room. Check that the password is empty and try again.',
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
    voiceInputDeviceLost: {
        id: 'app.multiplayer.mctier.voice.input-device-lost',
        defaultMessage: 'The microphone was disconnected. Reconnect it, then enable voice again.',
    },
    voiceOutputDeviceLost: {
        id: 'app.multiplayer.mctier.voice.output-device-lost',
        defaultMessage:
            'The speaker was disconnected. Voice is playing through the system default.',
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
    voiceActivation: {
        id: 'app.multiplayer.mctier.voice.activation',
        defaultMessage: 'Only send while I speak',
    },
    voiceActivationHint: {
        id: 'app.multiplayer.mctier.voice.activation-hint',
        defaultMessage: 'The microphone opens when you start talking and closes during silence.',
    },
    voiceNoiseReduction: {
        id: 'app.multiplayer.mctier.voice.noise-reduction',
        defaultMessage: 'AI noise reduction (RNNoise)',
    },
    voiceNoiseReductionNative: {
        id: 'app.multiplayer.mctier.voice.noise-reduction-native',
        defaultMessage: 'Your system already filters microphone noise.',
    },
    voiceNoiseReductionVirtualMic: {
        id: 'app.multiplayer.mctier.voice.noise-reduction-virtual-mic',
        defaultMessage: 'This microphone already removes noise, so it is left as is.',
    },
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
/** The checkbox follows a local value while the request is in flight: binding
 * it to the polled state alone left the box ticked after a rejected change,
 * because nothing in the binding changed to repaint it. */
const visibilityChecked = ref(false)
watch(
    () => props.state?.is_public,
    (isPublicNow) => {
        if (!visibilityPending.value) visibilityChecked.value = isPublicNow ?? false
    },
    { immediate: true },
)
const publishOutcome = computed(
    () => props.state?.publish_outcome ?? (props.state?.is_public ? 'published' : 'private'),
)
const visibilityOptions = [
    {
        id: 'mctier-visibility-private',
        inputName: 'public-lobby',
        value: false,
        icon: LockIcon,
        title: messages.visibilityPrivateTitle,
        description: messages.visibilityPrivateDescription,
    },
    {
        id: 'mctier-visibility-public',
        inputName: 'public-lobby',
        value: true,
        icon: GlobeIcon,
        title: messages.visibilityPublicTitle,
        description: messages.visibilityPublicDescription,
    },
]
const inviteLoading = ref(false)
const error = ref('')
/** Backend state errors carry transport detail; the UI shows one sentence. */
const displayError = computed(
    () => error.value || sanitizeErrorText(props.state?.error_message ?? ''),
)
/** The directory cannot list a password-protected room, so publishing clears
 * the password instead of offering a combination the service ignores. */
watch(isPublic, (isPublicNow) => {
    if (isPublicNow) password.value = ''
})
const busy = computed(
    () => pending.value || ['downloading', 'connecting'].includes(props.state?.status ?? ''),
)
const connected = computed(
    () =>
        props.state?.room_kind !== 'direct' &&
        ['hosting', 'joined'].includes(props.state?.status ?? ''),
)
/** Voice joins the peers in the room, so the panel is always configurable but
 * the call itself waits until the backend has a local player to signal from. */
const canStartVoice = computed(() => Boolean(props.state?.local_player_id))
const valid = computed(
    () =>
        isValidMctierLobbyName(lobbyName.value) &&
        isValidMctierPlayerName(playerName.value) &&
        validLocalPort(port.value) !== null &&
        isValidDirectServerNode(serverNode.value) &&
        isValidMctierSignalingServer(signalingServer.value),
)
/**
 * Why the room cannot be started yet, in the player's words.
 *
 * `valid` alone only greys out the button, and a disabled button never explains
 * itself: a first-time host saw "create does nothing" with no message. Every
 * branch here is a field the player can act on.
 */
const blockReason = computed(() => {
    if (!lobbyName.value.trim()) return formatMessage(messages.nameMissing)
    if (!isValidMctierLobbyName(lobbyName.value)) {
        return formatMessage(messages.nameInvalid)
    }
    if (!playerName.value.trim()) return formatMessage(messages.playerMissing)
    if (!isValidMctierPlayerName(playerName.value)) {
        return formatMessage(messages.playerInvalid)
    }
    if (validLocalPort(port.value) === null) return formatMessage(messages.portInvalid)
    if (!isValidDirectServerNode(serverNode.value)) return formatMessage(messages.nodeInvalid)
    if (!isValidMctierSignalingServer(signalingServer.value)) {
        return formatMessage(messages.signalingInvalid)
    }
    return ''
})
const playerLimit = computed(() => {
    const parsed = Number.parseInt(lobbyMaxPlayers.value, 10)
    if (!Number.isFinite(parsed) || parsed < 2) return null
    return Math.min(parsed, 100)
})
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

async function onInputDeviceChange(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLSelectElement)) return
    await voice.setInputDevice(target.value || null)
    // A device that fails to open is not committed, so show the one in use.
    target.value = voice.inputDeviceId.value ?? ''
}

async function onOutputDeviceChange(event: Event) {
    const target = event.target
    if (!(target instanceof HTMLSelectElement)) return
    await voice.setOutputDevice(target.value || null)
    // A sink that cannot be opened falls back to the system default, so the
    // select must show what is actually playing.
    target.value = voice.outputDeviceId.value ?? ''
}

function onVoiceActivationToggle(checked: boolean) {
    voice.setVoiceActivation(checked)
}

function onNoiseReductionToggle(checked: boolean) {
    void voice.setNoiseReduction(checked)
}

async function start(mode: 'create' | 'join', invite?: MctierInvite) {
    if (busy.value) return
    if (!valid.value) {
        // Report instead of returning silently: a click that does nothing is
        // indistinguishable from a crashed form.
        error.value = blockReason.value
        return
    }
    const nodeAddress = (invite?.server_node ?? serverNode.value).trim()
    if (!isValidDirectServerNode(nodeAddress)) {
        error.value = formatMessage(messages.nodeInvalid)
        return
    }
    const signalingAddress = (invite?.signaling_server ?? signalingServer.value).trim()
    if (!isValidMctierSignalingServer(signalingAddress)) {
        error.value = formatMessage(messages.signalingInvalid)
        return
    }
    // The invite supplies its own name, which never passed the form predicates.
    const roomName = invite?.lobby_name ?? lobbyName.value
    if (!isValidMctierLobbyName(roomName)) {
        error.value = formatMessage(messages.nameInvalid)
        return
    }
    if (!isValidMctierPlayerName(playerName.value)) {
        error.value = formatMessage(messages.playerInvalid)
        return
    }
    pending.value = true
    error.value = ''
    try {
        await multiplayer.startMctier({
            mode,
            lobby_name: roomName,
            password: invite?.password ?? password.value,
            player_name: playerName.value.trim(),
            server_node: nodeAddress,
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

async function updateVisibilityValue(next: boolean) {
    const previous = visibilityChecked.value
    visibilityChecked.value = next
    visibilityPending.value = true
    error.value = ''
    try {
        await multiplayer.setMctierPublic(next)
    } catch (failure) {
        // Put the box back where the backend still is, otherwise the user sees
        // "listed" for a room that was never published.
        visibilityChecked.value = previous
        error.value = multiplayerErrorText(failure)
    } finally {
        visibilityPending.value = false
    }
}

function onVisibilityToggle(checked: boolean) {
    void updateVisibilityValue(checked)
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
    // The panel is configurable before the room exists, so the lists have to be
    // filled on mount; waiting for a session left both selectors empty.
    void voice.refreshDevices()
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
    <section class="flex flex-col gap-4 rounded-xl bg-surface-2 p-5">
        <div>
            <h2 class="m-0 text-lg font-semibold">{{ formatMessage(messages.title) }}</h2>
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
        <p v-if="displayError" class="m-0 text-sm text-[var(--color-red)]" role="alert">
            {{ displayError }}
        </p>
        <section
            class="flex min-w-0 flex-col overflow-hidden rounded-lg border border-solid border-surface-4 bg-surface-3"
            data-onboarding-id="mctier-voice"
        >
            <div
                class="flex min-w-0 flex-wrap items-start justify-between gap-3 border-b border-surface-4 p-4"
            >
                <div class="min-w-0">
                    <h3 class="m-0 text-base font-semibold text-[var(--color-text-primary)]">
                        {{ formatMessage(messages.voiceTitle) }}
                    </h3>
                    <p class="m-0 mt-1 text-sm leading-relaxed text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.voiceDescription) }}
                    </p>
                </div>
                <div class="flex shrink-0 flex-wrap gap-2">
                    <Button
                        v-if="!voice.active.value"
                        type="outlined"
                        :disabled="voice.starting.value || !canStartVoice"
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
                                    voice.muted.value ? messages.voiceUnmute : messages.voiceMute,
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
            <p
                v-if="voice.active.value"
                class="m-0 border-b border-surface-4 px-4 py-3 text-sm"
                role="status"
            >
                {{
                    voice.connectedPeers.value
                        ? formatMessage(messages.voiceConnected, {
                              count: voice.connectedPeers.value,
                          })
                        : formatMessage(messages.voiceWaiting)
                }}
            </p>
            <SettingsRow>
                <template #label>{{ formatMessage(messages.voiceMicrophone) }}</template>
                <template #control>
                    <select
                        class="h-9 w-full min-w-0 rounded-xl border-none bg-surface-4 px-3 text-sm text-[var(--color-text-default)]"
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
                </template>
            </SettingsRow>
            <SettingsRow
                v-if="voice.outputDeviceSupported.value && voice.outputDevices.value.length"
            >
                <template #label>{{ formatMessage(messages.voiceSpeaker) }}</template>
                <template #control>
                    <select
                        class="h-9 w-full min-w-0 rounded-xl border-none bg-surface-4 px-3 text-sm text-[var(--color-text-default)]"
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
                </template>
            </SettingsRow>
            <!-- Session meters only make sense while a call is up; the device
                 pickers above stay outside this group so they can be set first. -->
            <!-- Without Web Audio there is no gain node, so the microphone
                 level cannot be scaled; showing the slider would offer a
                 control that silently does nothing. -->
            <SettingsRow v-if="voice.active.value && voice.inputVolumeSupported.value" stacked>
                <template #label>{{ formatMessage(messages.voiceInputVolume) }}</template>
                <template #control>
                    <div class="flex w-full min-w-0 items-center gap-3">
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
                        <span
                            class="w-10 shrink-0 text-right text-sm tabular-nums text-[var(--color-text-tertiary)]"
                        >
                            {{ Math.round(voice.inputVolume.value * 100) }}%
                        </span>
                    </div>
                </template>
            </SettingsRow>
            <!-- The meter is fed by an AnalyserNode, so it needs the same Web
                 Audio support as the gain node; otherwise it would sit at zero
                 and read as "your microphone is broken". -->
            <SettingsRow v-if="voice.active.value && voice.inputVolumeSupported.value" stacked>
                <template #label>{{ formatMessage(messages.voiceMicLevel) }}</template>
                <template #control>
                    <div
                        class="h-1.5 w-full min-w-0 overflow-hidden rounded bg-surface-5"
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
                </template>
            </SettingsRow>
            <SettingsRow v-if="voice.active.value" stacked>
                <template #label>{{ formatMessage(messages.voiceVolume) }}</template>
                <template #control>
                    <div class="flex w-full min-w-0 items-center gap-3">
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
                        <span
                            class="w-10 shrink-0 text-right text-sm tabular-nums text-[var(--color-text-tertiary)]"
                        >
                            {{ Math.round(voice.outputVolume.value * 100) }}%
                        </span>
                    </div>
                </template>
            </SettingsRow>
            <SettingsRow v-if="voice.voiceActivationSupported.value" inline-control>
                <template #label>{{ formatMessage(messages.voiceActivation) }}</template>
                <template #description>
                    {{ formatMessage(messages.voiceActivationHint) }}
                </template>
                <template #control>
                    <Checkbox
                        :model-value="voice.voiceActivation.value"
                        :description="formatMessage(messages.voiceActivation)"
                        @update:model-value="onVoiceActivationToggle"
                    />
                </template>
            </SettingsRow>
            <!-- RNNoise is the only mode worth a switch. `native` means the
                 browser already filters, so there is nothing to toggle and
                 only a note is shown; `unavailable` hides the row entirely. -->
            <SettingsRow v-if="voice.noiseSuppressionMode.value === 'rnnoise'" inline-control>
                <template #label>{{ formatMessage(messages.voiceNoiseReduction) }}</template>
                <template v-if="voice.noiseReductionForcedNative.value" #description>
                    {{ formatMessage(messages.voiceNoiseReductionVirtualMic) }}
                </template>
                <template #control>
                    <Checkbox
                        :model-value="voice.noiseReduction.value"
                        :disabled="voice.noiseReductionForcedNative.value"
                        :description="formatMessage(messages.voiceNoiseReduction)"
                        @update:model-value="onNoiseReductionToggle"
                    />
                </template>
            </SettingsRow>
            <p
                v-else-if="voice.noiseSuppressionMode.value === 'native'"
                class="m-0 px-4 py-3 text-sm leading-relaxed text-[var(--color-text-tertiary)]"
            >
                {{ formatMessage(messages.voiceNoiseReductionNative) }}
            </p>
            <p
                v-if="voice.error.value"
                class="m-0 px-4 py-3 text-sm leading-relaxed text-[var(--color-red)]"
                role="alert"
            >
                {{
                    formatMessage(
                        voice.error.value === 'permission'
                            ? messages.voiceErrorPermission
                            : voice.error.value === 'unsupported'
                              ? messages.voiceErrorUnsupported
                              : voice.error.value === 'input-device-lost'
                                ? messages.voiceInputDeviceLost
                                : voice.error.value === 'output-device-lost'
                                  ? messages.voiceOutputDeviceLost
                                  : messages.voiceErrorUnavailable,
                    )
                }}
            </p>
        </section>
        <template v-if="connected">
            <CopyCode v-if="state?.lobby_name" :text="state.lobby_name" />
            <template v-if="state?.mode === 'create'">
                <div
                    class="flex min-w-0 flex-col gap-3 rounded-lg border border-solid border-surface-4 bg-surface-3 p-4"
                    data-onboarding-id="mctier-publish-state"
                >
                    <div class="flex min-w-0 items-start justify-between gap-3">
                        <div class="min-w-0">
                            <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                                {{ formatMessage(messages.visibilityTitle) }}
                            </h3>
                            <p class="m-0 mt-1 text-xs text-[var(--color-text-tertiary)]">
                                {{
                                    formatMessage(
                                        state.is_public
                                            ? messages.visibilityPublicDescription
                                            : messages.visibilityPrivateDescription,
                                    )
                                }}
                            </p>
                        </div>
                        <label class="flex shrink-0 items-center gap-2 text-sm">
                            <Checkbox
                                :model-value="visibilityChecked"
                                :disabled="visibilityPending"
                                @update:model-value="onVisibilityToggle"
                            />
                            <span>{{ formatMessage(messages.visibility) }}</span>
                        </label>
                    </div>
                    <p
                        class="m-0 flex flex-wrap items-center gap-2 text-xs"
                        data-onboarding-id="mctier-publish-badge"
                        role="status"
                    >
                        <span
                            class="rounded-full px-2 py-0.5 font-semibold"
                            :class="
                                publishOutcome === 'published'
                                    ? 'bg-brand-highlight text-brand'
                                    : publishOutcome === 'not_listed'
                                      ? 'bg-surface-4 text-red'
                                      : publishOutcome === 'sent_unconfirmed'
                                        ? 'bg-surface-4 text-[var(--color-text-tertiary)]'
                                        : 'bg-surface-4 text-[var(--color-text-tertiary)]'
                            "
                        >
                            {{
                                formatMessage(
                                    publishOutcome === 'published'
                                        ? messages.publishPublished
                                        : publishOutcome === 'not_listed'
                                          ? messages.publishNotListed
                                          : publishOutcome === 'sent_unconfirmed'
                                            ? messages.publishUnconfirmed
                                            : messages.publishPrivate,
                                )
                            }}
                        </span>
                        <span v-if="visibilityPending" class="text-[var(--color-text-tertiary)]">
                            {{ formatMessage(messages.publicPending) }}
                        </span>
                    </p>
                    <p
                        v-if="state.has_password"
                        class="m-0 text-xs text-[var(--color-text-tertiary)]"
                    >
                        {{ formatMessage(messages.publicWithPasswordHint) }}
                    </p>
                </div>
                <div
                    v-if="inviteCode"
                    class="flex min-w-0 flex-col gap-2"
                    data-onboarding-id="mctier-invite"
                >
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
                            class="w-32"
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
            <Button type="outlined" color="red" :disabled="stopping" @click="stop">
                <XIcon />
                {{ formatMessage(messages.leave) }}
            </Button>
        </template>
        <template v-else-if="busy">
            <Button type="outlined" :disabled="stopping" @click="stop">{{
                formatMessage(messages.cancel)
            }}</Button>
        </template>
        <form
            v-else
            class="grid gap-3 sm:grid-cols-2"
            data-onboarding-id="mctier-create-form"
            @submit.prevent="start('create')"
        >
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.name) }}
                <div class="flex min-w-0 items-center gap-2">
                    <StyledInput v-model="lobbyName" name="lobby-name" />
                    <Button type="quiet" size="sm" @click="generateLobbyName">{{
                        formatMessage(messages.randomName)
                    }}</Button>
                </div>
            </label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.player)
                }}<StyledInput v-model="playerName" name="player-name"
            /></label>
            <label class="flex flex-col gap-1 text-sm">
                <span>{{ formatMessage(messages.password) }}</span>
                <div class="flex min-w-0 items-center gap-2">
                    <StyledInput
                        v-model="password"
                        name="lobby-password"
                        type="password"
                        :maxlength="128"
                        :disabled="isPublic"
                    />
                    <Button type="quiet" size="sm" :disabled="isPublic" @click="generatePassword">{{
                        formatMessage(messages.randomPassword)
                    }}</Button>
                </div>
                <span v-if="isPublic" class="text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.publicNoPassword) }}
                </span>
            </label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.port)
                }}<StyledInput v-model="port" name="game-port" inputmode="numeric"
            /></label>
            <label class="flex flex-col gap-1 text-sm"
                >{{ formatMessage(messages.node)
                }}<StyledInput v-model="serverNode" name="server-node" /><span
                    v-if="serverNode.trim() && !isValidDirectServerNode(serverNode)"
                    class="text-xs text-[var(--color-red)]"
                >
                    {{ formatMessage(messages.nodeInvalid) }}
                </span></label
            >
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
            <fieldset
                class="m-0 flex min-w-0 flex-col gap-2 border-0 p-0 sm:col-span-2"
                data-onboarding-id="mctier-visibility"
            >
                <legend class="mb-1 p-0 text-sm font-semibold">
                    {{ formatMessage(messages.visibilityTitle) }}
                </legend>
                <div class="grid gap-3 sm:grid-cols-2">
                    <label
                        v-for="option in visibilityOptions"
                        :key="option.id"
                        :data-onboarding-id="option.id"
                        class="flex min-w-0 cursor-pointer items-start gap-2 rounded-md border border-solid p-3 text-left"
                        :class="
                            isPublic === option.value
                                ? 'border-brand bg-surface-3'
                                : 'border-surface-4'
                        "
                    >
                        <input
                            type="radio"
                            class="mt-1 h-4 w-4 shrink-0"
                            :name="option.inputName"
                            :checked="isPublic === option.value"
                            @change="isPublic = option.value"
                        />
                        <span class="min-w-0">
                            <span
                                class="flex items-center gap-1 text-sm font-semibold text-[var(--color-text-primary)]"
                            >
                                <component :is="option.icon" class="size-4 shrink-0 text-brand" />
                                {{ formatMessage(option.title) }}
                            </span>
                            <span class="block text-xs text-[var(--color-text-tertiary)]">
                                {{ formatMessage(option.description) }}
                            </span>
                        </span>
                    </label>
                </div>
            </fieldset>
            <p
                v-if="!isPublic && password.trim()"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
                data-onboarding-id="mctier-password"
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
            <p
                v-if="blockReason"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
                role="status"
            >
                {{ blockReason }}
                <button
                    v-if="!lobbyName.trim()"
                    type="button"
                    class="cursor-pointer border-0 bg-transparent p-0 text-sm font-semibold text-brand underline"
                    @click="generateLobbyName"
                >
                    {{ formatMessage(messages.randomName) }}
                </button>
            </p>
            <div class="flex flex-wrap gap-2 sm:col-span-2">
                <Button :disabled="!valid" @click="start('create')">{{
                    formatMessage(messages.host)
                }}</Button>
                <Button :disabled="!valid" @click="start('join')">{{
                    formatMessage(messages.join)
                }}</Button>
            </div>
        </form>
        <form
            v-if="!connected && !busy"
            class="flex min-w-0 flex-col gap-2 border-t border-solid border-surface-4 pt-4"
            @submit.prevent="joinByInvite"
        >
            <label class="flex min-w-0 flex-col gap-1 text-sm">
                {{ formatMessage(messages.inviteCode) }}
                <StyledInput
                    v-model="inviteCodeInput"
                    name="mctier-invite-code"
                    autocomplete="off"
                />
            </label>
            <Button :disabled="busy || !inviteCodeInput.trim()" native-type="submit">{{
                formatMessage(messages.joinByInvite)
            }}</Button>
        </form>
    </section>
</template>
