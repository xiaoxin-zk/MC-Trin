<script setup lang="ts">
import { MicrophoneIcon, XIcon } from '@modrinth/assets'
import { Button, CopyCode, defineMessages, StyledInput, useVIntl } from '@modrinth/ui'
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
    publicPasswordHint: {
        id: 'app.multiplayer.mctier.public-password-hint',
        defaultMessage: 'Public rooms cannot have a password.',
    },
    privatePasswordHint: {
        id: 'app.multiplayer.mctier.private-password-hint',
        defaultMessage: 'Set a password to keep this room private.',
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
const canCreate = computed(
    () => valid.value && (isPublic.value ? !password.value.trim() : Boolean(password.value.trim())),
)
const status = computed(() => {
    const value = props.state?.status
    return value && value in messages ? formatMessage(messages[value as 'hosting']) : ''
})
const voice = useMctierVoice(multiplayer.sendMctierVoiceSignal)
let unlistenVoice: UnlistenFn | undefined
let componentUnmounted = false

function startVoice() {
    if (props.state) void voice.start(props.state)
}

async function start(mode: 'create' | 'join', invite?: MctierInvite) {
    if (!valid.value || busy.value) return
    if (mode === 'create' && isPublic.value && password.value.trim()) {
        error.value = formatMessage(messages.publicPasswordHint)
        return
    }
    if (mode === 'create' && !isPublic.value && !password.value.trim()) {
        error.value = formatMessage(messages.privatePasswordHint)
        return
    }
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

watch(
    () => [props.state?.status, props.state?.mode] as const,
    ([status, mode]) => {
        if (status === 'hosting' && mode === 'create') {
            void refreshInvite()
        } else {
            inviteCode.value = ''
        }
    },
    { immediate: true },
)

watch(() => props.state, voice.syncRoom, { deep: true })

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
                        :disabled="visibilityPending || state.has_password"
                        @change="updateVisibility"
                    />
                    <span>{{ formatMessage(messages.visibility) }}</span>
                    <span v-if="visibilityPending" role="status">
                        {{ formatMessage(messages.publicPending) }}
                    </span>
                </label>
                <p v-if="state.has_password" class="m-0 text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.publicPasswordHint) }}
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
                    class="flex flex-wrap justify-between gap-2 py-1 text-sm"
                >
                    <span>{{ player.playerName }}</span
                    ><span class="text-[var(--color-text-tertiary)]">{{ player.virtualIp }}</span>
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
            <label class="flex items-center gap-2 text-sm sm:col-span-2">
                <input v-model="isPublic" name="public-lobby" type="checkbox" />
                <span>{{ formatMessage(messages.visibility) }}</span>
            </label>
            <p
                v-if="isPublic && password.trim()"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
            >
                {{ formatMessage(messages.publicPasswordHint) }}
            </p>
            <p
                v-else-if="!isPublic && !password.trim()"
                class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2"
            >
                {{ formatMessage(messages.privatePasswordHint) }}
            </p>
            <p class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2">
                {{ formatMessage(messages.hostHint) }}
            </p>
            <div class="flex flex-wrap gap-2 sm:col-span-2">
                <Button :disabled="!canCreate" @click="start('create')">{{
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
