<script setup lang="ts">
import { LockIcon } from '@modrinth/assets'
import { Button, CopyCode, defineMessages, StyledInput, useVIntl } from '@modrinth/ui'
import { computed, ref } from 'vue'

import { DEFAULT_MCTIER_NODE, type DirectRoomRequest, type MctierState } from '@/helpers/mctier'
import { multiplayer, multiplayerErrorText, validLocalPort } from '@/helpers/multiplayer'

const props = defineProps<{ state?: MctierState }>()

const { formatMessage } = useVIntl()

const messages = defineMessages({
    title: { id: 'app.multiplayer.direct.title', defaultMessage: 'Direct room' },
    description: {
        id: 'app.multiplayer.direct.description',
        defaultMessage:
            'A password-protected room with no lobby service behind it. Share the room code; nothing is listed publicly.',
    },
    roomName: { id: 'app.multiplayer.direct.room-name', defaultMessage: 'Room name' },
    password: { id: 'app.multiplayer.direct.password', defaultMessage: 'Room password' },
    playerName: { id: 'app.multiplayer.direct.player-name', defaultMessage: 'Player name' },
    port: { id: 'app.multiplayer.direct.port', defaultMessage: 'Minecraft port' },
    node: { id: 'app.multiplayer.direct.node', defaultMessage: 'EasyTier node' },
    joinAddress: {
        id: 'app.multiplayer.direct.join-address',
        defaultMessage: 'Host address (only when joining)',
    },
    joinAddressHint: {
        id: 'app.multiplayer.direct.join-address-hint',
        defaultMessage: 'Ask the host for the address shown in their room, such as 10.126.126.7',
    },
    create: { id: 'app.multiplayer.direct.create', defaultMessage: 'Create room' },
    join: { id: 'app.multiplayer.direct.join', defaultMessage: 'Join room' },
    leave: { id: 'app.multiplayer.direct.leave', defaultMessage: 'Leave room' },
    hostHint: {
        id: 'app.multiplayer.direct.host-hint',
        defaultMessage:
            'Open your world to LAN first and enter the port Minecraft shows. Guests join by the address below.',
    },
    address: {
        id: 'app.multiplayer.direct.address',
        defaultMessage: 'Give this address to your guests',
    },
    roomCode: { id: 'app.multiplayer.direct.room-code', defaultMessage: 'Room code' },
    roomCodeHint: {
        id: 'app.multiplayer.direct.room-code-hint',
        defaultMessage:
            'It bundles the room name, password, node, and port so a guest can paste it once.',
    },
    invalid: {
        id: 'app.multiplayer.direct.invalid',
        defaultMessage: 'Room names use 4-32 letters, numbers, underscores, or hyphens.',
    },
    needsPassword: {
        id: 'app.multiplayer.direct.needs-password',
        defaultMessage: 'A direct room needs a password.',
    },
    starting: { id: 'app.multiplayer.direct.starting', defaultMessage: 'Preparing the room…' },
})

const roomName = ref('')
const password = ref('')
const playerName = ref('Player')
const port = ref('25565')
const node = ref(DEFAULT_MCTIER_NODE)
const joinAddress = ref('')
const pending = ref(false)
const error = ref('')

const connected = computed(() => ['hosting', 'joined'].includes(props.state?.status ?? ''))
const roomCode = computed(() => {
    const state = props.state
    if (!state?.lobby_name || !state.virtual_ip) return ''
    // Keep the same shape the lobby invite uses so one paste handler can read
    // either kind of room.
    const params = new URLSearchParams({
        name: state.lobby_name,
        node: node.value.trim(),
        port: String(state.game_port ?? 0),
    })
    if (state.virtual_ip) params.set('host', state.virtual_ip)
    return `mctrin-direct://room?${params.toString()}`
})

const valid = computed(
    () =>
        /^[\p{L}\p{N}_-]{4,32}$/u.test(roomName.value) &&
        playerName.value.trim().length > 0 &&
        password.value.trim().length > 0 &&
        validLocalPort(port.value) !== null,
)

async function start(joining: boolean) {
    if (!valid.value || pending.value) return
    const parsedPort = validLocalPort(port.value)
    if (parsedPort === null) return
    pending.value = true
    error.value = ''
    try {
        const request: DirectRoomRequest = {
            room_name: roomName.value,
            password: password.value,
            player_name: playerName.value.trim(),
            server_node: node.value.trim(),
            game_port: parsedPort,
            host_address: joining ? joinAddress.value.trim() || null : null,
        }
        await multiplayer.startDirect(request)
    } catch (failure) {
        error.value = multiplayerErrorText(failure)
    } finally {
        pending.value = false
    }
}

async function stop() {
    try {
        await multiplayer.stop()
    } catch (failure) {
        error.value = multiplayerErrorText(failure)
    }
}
</script>

<template>
    <section class="flex flex-col gap-4 rounded-md bg-surface-1 p-5">
        <div>
            <h2 class="m-0 flex items-center gap-2 text-xl font-semibold">
                <LockIcon class="size-5 text-brand" />
                {{ formatMessage(messages.title) }}
            </h2>
            <p class="mb-0 text-sm text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.description) }}
            </p>
        </div>

        <p v-if="error || state?.error_message" class="m-0 text-sm text-red" role="alert">
            {{ error || state?.error_message }}
        </p>

        <template v-if="connected">
            <p class="m-0 text-sm">{{ formatMessage(messages.address) }}</p>
            <CopyCode v-if="state?.connect_address" :text="state.connect_address" />
            <div v-if="roomCode" class="flex flex-col gap-2">
                <h3 class="m-0 text-base font-semibold">{{ formatMessage(messages.roomCode) }}</h3>
                <CopyCode :text="roomCode" />
                <p class="m-0 text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.roomCodeHint) }}
                </p>
            </div>
            <Button @click="stop">{{ formatMessage(messages.leave) }}</Button>
        </template>

        <form v-else class="grid gap-3 sm:grid-cols-2" @submit.prevent="start(false)">
            <label class="flex flex-col gap-1 text-sm">
                <span>{{ formatMessage(messages.roomName) }}</span>
                <StyledInput v-model="roomName" name="direct-room-name" :maxlength="32" />
            </label>
            <label class="flex flex-col gap-1 text-sm">
                <span>{{ formatMessage(messages.playerName) }}</span>
                <StyledInput v-model="playerName" name="direct-player-name" :maxlength="64" />
            </label>
            <label class="flex flex-col gap-1 text-sm">
                <span>{{ formatMessage(messages.password) }}</span>
                <StyledInput
                    v-model="password"
                    name="direct-password"
                    type="password"
                    :maxlength="128"
                />
                <span v-if="!password.trim()" class="text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.needsPassword) }}
                </span>
            </label>
            <label class="flex flex-col gap-1 text-sm">
                <span>{{ formatMessage(messages.port) }}</span>
                <StyledInput v-model="port" name="direct-port" inputmode="numeric" />
            </label>
            <label class="flex flex-col gap-1 text-sm sm:col-span-2">
                <span>{{ formatMessage(messages.node) }}</span>
                <StyledInput v-model="node" name="direct-node" />
            </label>
            <label class="flex flex-col gap-1 text-sm sm:col-span-2">
                <span>{{ formatMessage(messages.joinAddress) }}</span>
                <StyledInput v-model="joinAddress" name="direct-join-address" />
                <span class="text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.joinAddressHint) }}
                </span>
            </label>
            <p class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2">
                {{ formatMessage(messages.hostHint) }}
            </p>
            <div class="flex flex-wrap gap-2 sm:col-span-2">
                <Button :disabled="!valid || pending" @click="start(false)">
                    {{ formatMessage(messages.create) }}
                </Button>
                <Button
                    type="outlined"
                    :disabled="!valid || pending || !joinAddress.trim()"
                    @click="start(true)"
                >
                    {{ formatMessage(messages.join) }}
                </Button>
            </div>
            <p v-if="pending" class="m-0 text-sm sm:col-span-2" role="status">
                {{ formatMessage(messages.starting) }}
            </p>
        </form>
    </section>
</template>
