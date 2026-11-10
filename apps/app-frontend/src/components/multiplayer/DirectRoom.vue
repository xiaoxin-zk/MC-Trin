<script setup lang="ts">
import { LockIcon } from '@modrinth/assets'
import { Button, CopyCode, defineMessages, StyledInput, useVIntl } from '@modrinth/ui'
import { computed, ref } from 'vue'

import {
    DEFAULT_MCTIER_NODE,
    type DirectRoomRequest,
    type MctierState,
    isValidDirectServerNode,
    parseDirectRoomCode,
} from '@/helpers/mctier'
import { multiplayer, multiplayerErrorText, sanitizeErrorText, validLocalPort } from '@/helpers/multiplayer'

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
    guestPort: {
        id: 'app.multiplayer.direct.guest-port',
        defaultMessage: 'Host Minecraft port',
    },
    node: { id: 'app.multiplayer.direct.node', defaultMessage: 'EasyTier node' },
    joinAddress: {
        id: 'app.multiplayer.direct.join-address',
        defaultMessage: 'Host address or room code',
    },
    joinAddressHint: {
        id: 'app.multiplayer.direct.join-address-hint',
        defaultMessage:
            'Paste the room code the host shared, or type the host address shown in their room, such as 10.126.126.7.',
    },
    create: { id: 'app.multiplayer.direct.create', defaultMessage: 'Create room' },
    join: { id: 'app.multiplayer.direct.join', defaultMessage: 'Join room' },
    leave: { id: 'app.multiplayer.direct.leave', defaultMessage: 'Leave room' },
    cancel: { id: 'app.multiplayer.direct.cancel', defaultMessage: 'Cancel' },
    hostHint: {
        id: 'app.multiplayer.direct.host-hint',
        defaultMessage:
            'Open your world to LAN first and enter the port Minecraft shows. Guests join by the address below.',
    },
    guestHint: {
        id: 'app.multiplayer.direct.guest-hint',
        defaultMessage:
            'Enter the host’s LAN port and address. The host must keep the world open to LAN.',
    },
    address: {
        id: 'app.multiplayer.direct.address',
        defaultMessage: 'Give this address to your guests',
    },
    guestAddress: {
        id: 'app.multiplayer.direct.guest-address',
        defaultMessage: 'Open this address in Minecraft',
    },
    roomCode: { id: 'app.multiplayer.direct.room-code', defaultMessage: 'Room code' },
    roomCodeHint: {
        id: 'app.multiplayer.direct.room-code-hint',
        defaultMessage:
            'It bundles the room name, node, host address, and port. The password is never part of it — share it separately.',
    },
    invalidCode: {
        id: 'app.multiplayer.direct.invalid-code',
        defaultMessage: 'That is not a direct room code or host address.',
    },
    invalidNode: {
        id: 'app.multiplayer.direct.invalid-node',
        defaultMessage: 'Enter a node address such as tcp://node.example:11010.',
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
    gamePortClosed: {
        id: 'app.multiplayer.direct.game-port-closed',
        defaultMessage:
            'Minecraft is not listening on port {port} yet. Open the world to LAN in game so friends can join.',
    },
})

const roomName = ref('')
const password = ref('')
const playerName = ref('Player')
const port = ref('25565')
const node = ref(DEFAULT_MCTIER_NODE)
const joinAddress = ref('')
const pending = ref(false)
const stopping = ref(false)
const error = ref('')
/** Backend state errors carry transport detail; the UI shows one sentence. */
const displayError = computed(() => error.value || sanitizeErrorText(props.state?.error_message ?? ''))

/** The room this component created or joined, so the other provider's session
 * is never rendered as this one's. */
const ownsRoom = computed(() => props.state?.room_kind !== 'lobby')
const connected = computed(
    () => ownsRoom.value && ['hosting', 'joined'].includes(props.state?.status ?? ''),
)
const joiningRoom = computed(() => props.state?.mode === 'join')
const roomCode = computed(() => {
    const state = props.state
    // Only the host can hand out a code: for a guest the virtual address is
    // their own, so a code built here would point other players at themselves.
    if (joiningRoom.value || !ownsRoom.value || !state?.lobby_name || !state.virtual_ip) return ''
    // Keep the same shape the lobby invite uses so one paste handler can read
    // either kind of room.
    const params = new URLSearchParams({
        name: state.lobby_name,
        node: node.value.trim(),
        port: String(state.game_port ?? 0),
    })
    params.set('host', state.virtual_ip)
    return `mctrin-direct://room?${params.toString()}`
})

const baseValid = computed(
    () =>
        playerName.value.trim().length > 0 &&
        password.value.trim().length > 0 &&
        validLocalPort(port.value) !== null &&
        isValidDirectServerNode(node.value),
)
/** Creating needs the room definition the host types. */
const canCreate = computed(
    () => baseValid.value && /^[\p{L}\p{N}_-]{4,32}$/u.test(roomName.value),
)
/**
 * Joining needs a room code or host address, not a typed room name: the code
 * itself carries the name, so requiring both would force the guest to press
 * the decode button before "Join room" became usable.
 */
const canJoin = computed(
    () => baseValid.value && parseDirectRoomCode(joinAddress.value) !== null,
)

async function start(joining: boolean) {
    if (pending.value) return
    if (!(joining ? canJoin.value : canCreate.value)) return
    const parsedPort = validLocalPort(port.value)
    if (parsedPort === null) return
    if (!isValidDirectServerNode(node.value)) {
        error.value = formatMessage(messages.invalidNode)
        return
    }
    if (!joining && !/^[\p{L}\p{N}_-]{4,32}$/u.test(roomName.value)) {
        error.value = formatMessage(messages.invalid)
        return
    }
    let hostAddress: string | null = null
    if (joining) {
        // A pasted room code is only expanded here, so "Join room" works
        // whether or not the guest pressed the code button first.
        const parsed = parseDirectRoomCode(joinAddress.value)
        if (!parsed) {
            error.value = formatMessage(messages.invalidCode)
            return
        }
        hostAddress = parsed.host_address
        joinAddress.value = parsed.host_address
        if (parsed.room_name && !roomName.value.trim()) roomName.value = parsed.room_name
        if (parsed.server_node) node.value = parsed.server_node
        if (parsed.game_port !== undefined) port.value = String(parsed.game_port)
    }
    pending.value = true
    error.value = ''
    try {
        const request: DirectRoomRequest = {
            room_name: roomName.value,
            password: password.value,
            player_name: playerName.value.trim(),
            server_node: node.value.trim(),
            game_port: validLocalPort(port.value) ?? parsedPort,
            host_address: hostAddress,
        }
        await multiplayer.startDirect(request)
    } catch (failure) {
        error.value = multiplayerErrorText(failure)
    } finally {
        pending.value = false
    }
}

/** Accept either a full room code or the bare address the host reads out. */
function applyRoomCode() {
    const parsed = parseDirectRoomCode(joinAddress.value)
    if (!parsed) {
        error.value = formatMessage(messages.invalidCode)
        return
    }
    error.value = ''
    joinAddress.value = parsed.host_address
    if (parsed.room_name) roomName.value = parsed.room_name
    if (parsed.server_node) node.value = parsed.server_node
    if (parsed.game_port !== undefined) port.value = String(parsed.game_port)
}

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

        <p v-if="displayError" class="m-0 text-sm text-red" role="alert">
            {{ displayError }}
        </p>

        <template v-if="connected">
            <p class="m-0 text-sm">
                {{ formatMessage(joiningRoom ? messages.guestAddress : messages.address) }}
            </p>
            <CopyCode v-if="state?.connect_address" :text="state.connect_address" />
            <p
                v-if="state?.host_hint === 'game_port_closed'"
                class="m-0 text-xs text-[var(--color-text-tertiary)]"
            >
                {{ formatMessage(messages.gamePortClosed, { port: state?.game_port ?? 0 }) }}
            </p>
            <div v-if="roomCode" class="flex flex-col gap-2">
                <h3 class="m-0 text-base font-semibold">{{ formatMessage(messages.roomCode) }}</h3>
                <CopyCode :text="roomCode" />
                <p class="m-0 text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.roomCodeHint) }}
                </p>
            </div>
            <Button :disabled="stopping" @click="stop">{{ formatMessage(messages.leave) }}</Button>
        </template>

        <template
            v-else-if="
                pending ||
                (ownsRoom && ['downloading', 'connecting'].includes(state?.status ?? ''))
            "
        >
            <p class="m-0 text-sm" role="status">{{ formatMessage(messages.starting) }}</p>
            <Button :disabled="stopping" @click="stop">{{ formatMessage(messages.cancel) }}</Button>
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
                <span>{{ formatMessage(joinAddress.trim() ? messages.guestPort : messages.port) }}</span>
                <StyledInput v-model="port" name="direct-port" inputmode="numeric" />
            </label>
            <label class="flex flex-col gap-1 text-sm sm:col-span-2">
                <span>{{ formatMessage(messages.node) }}</span>
                <StyledInput v-model="node" name="direct-node" />
                <span
                    v-if="node.trim() && !isValidDirectServerNode(node)"
                    class="text-xs text-[var(--color-red)]"
                >
                    {{ formatMessage(messages.invalidNode) }}
                </span>
            </label>
            <label class="flex flex-col gap-1 text-sm sm:col-span-2">
                <span>{{ formatMessage(messages.joinAddress) }}</span>
                <div class="flex min-w-0 items-center gap-2">
                    <StyledInput v-model="joinAddress" name="direct-join-address" />
                    <Button type="quiet" size="sm" @click="applyRoomCode">{{
                        formatMessage(messages.roomCode)
                    }}</Button>
                </div>
                <span class="text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.joinAddressHint) }}
                </span>
            </label>
            <p class="m-0 text-sm text-[var(--color-text-tertiary)] sm:col-span-2">
                {{ formatMessage(joinAddress.trim() ? messages.guestHint : messages.hostHint) }}
            </p>
            <div class="flex flex-wrap gap-2 sm:col-span-2">
                <Button :disabled="!canCreate || pending" @click="start(false)">
                    {{ formatMessage(messages.create) }}
                </Button>
                <Button type="outlined" :disabled="!canJoin || pending" @click="start(true)">
                    {{ formatMessage(messages.join) }}
                </Button>
            </div>
        </form>
    </section>
</template>
