<script setup lang="ts">
import { GlobeIcon, LogInIcon, RefreshCwIcon, SpinnerIcon, UsersIcon } from '@modrinth/assets'
import { Admonition, Button, defineMessages, useVIntl } from '@modrinth/ui'
import { onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { multiplayer, multiplayerErrorText, validLocalPort } from '@/helpers/multiplayer'
import {
    buildPublicLobbyJoinRequest,
    fetchPublicLobbies,
    type PublicLobby,
} from '@/helpers/public-lobbies'

const { formatMessage } = useVIntl()
const messages = defineMessages({
    title: { id: 'app.multiplayer.public-lobbies.title', defaultMessage: 'Public lobbies' },
    description: {
        id: 'app.multiplayer.public-lobbies.description',
        defaultMessage: 'Browse rooms currently shared on the public directory.',
    },
    joinHint: {
        id: 'app.multiplayer.public-lobbies.join-hint',
        defaultMessage:
            'Choose a room to join. The directory never supplies a password or Minecraft port, so those values stay in your control.',
    },
    refresh: { id: 'app.multiplayer.public-lobbies.refresh', defaultMessage: 'Refresh' },
    loading: {
        id: 'app.multiplayer.public-lobbies.loading',
        defaultMessage: 'Loading public lobbies…',
    },
    error: {
        id: 'app.multiplayer.public-lobbies.error',
        defaultMessage: 'Could not load public lobbies. Check your connection and try again.',
    },
    timeout: {
        id: 'app.multiplayer.public-lobbies.timeout',
        defaultMessage: 'The public lobby directory timed out. Try again.',
    },
    empty: {
        id: 'app.multiplayer.public-lobbies.empty',
        defaultMessage: 'No public lobbies are available right now.',
    },
    createRoom: {
        id: 'app.multiplayer.public-lobbies.create-room',
        defaultMessage: 'Create a room',
    },
    host: { id: 'app.multiplayer.public-lobbies.host', defaultMessage: 'Hosted by {name}' },
    playerCount: {
        id: 'app.multiplayer.public-lobbies.player-count',
        defaultMessage: '{count} players',
    },
    playerCapacity: {
        id: 'app.multiplayer.public-lobbies.player-capacity',
        defaultMessage: '{count} / {max} players',
    },
    join: { id: 'app.multiplayer.public-lobbies.join', defaultMessage: 'Join lobby' },
    playerName: {
        id: 'app.multiplayer.public-lobbies.player-name',
        defaultMessage: 'Player name',
    },
    gamePort: {
        id: 'app.multiplayer.public-lobbies.game-port',
        defaultMessage: 'Minecraft LAN port',
    },
    password: {
        id: 'app.multiplayer.public-lobbies.password',
        defaultMessage: 'Lobby password (optional)',
    },
    portHint: {
        id: 'app.multiplayer.public-lobbies.port-hint',
        defaultMessage: 'Open your world to LAN first, then enter the port Minecraft shows.',
    },
    invalidPort: {
        id: 'app.multiplayer.public-lobbies.invalid-port',
        defaultMessage: 'Minecraft port must be between 1 and 65535.',
    },
    cancel: { id: 'app.multiplayer.public-lobbies.cancel', defaultMessage: 'Cancel' },
    connecting: {
        id: 'app.multiplayer.public-lobbies.connecting',
        defaultMessage: 'Connecting to {name}...',
    },
})

const router = useRouter()
const lobbies = ref<PublicLobby[]>([])
const loading = ref(false)
const loaded = ref(false)
const failed = ref(false)
const timedOut = ref(false)
const request = new AbortController()
const selectedLobby = ref<PublicLobby | null>(null)
const playerName = ref('Player')
const gamePort = ref('25565')
const password = ref('')
const joinError = ref('')
const joining = ref(false)

function openJoin(lobby: PublicLobby) {
    if (lobby.maxPlayers !== null && lobby.playerCount >= lobby.maxPlayers) return
    selectedLobby.value = lobby
    joinError.value = ''
}

function closeJoin() {
    if (joining.value) return
    selectedLobby.value = null
    joinError.value = ''
}

async function joinSelected() {
    const lobby = selectedLobby.value
    if (!lobby || joining.value) return
    const parsedPort = validLocalPort(gamePort.value)
    if (parsedPort === null) {
        joinError.value = formatMessage(messages.invalidPort)
        return
    }

    joining.value = true
    joinError.value = ''
    try {
        const request = buildPublicLobbyJoinRequest(
            lobby,
            playerName.value,
            parsedPort,
            password.value,
        )
        // The backend owns the real signaling/EasyTier lifecycle. It stays
        // pending while connected; the Rooms page polls authoritative state.
        void multiplayer.startMctier(request).catch((error: unknown) => {
            joinError.value = multiplayerErrorText(error)
            joining.value = false
        })
        await router.push({ path: '/multiplayer/rooms', query: { provider: 'mctier' } })
    } catch (error: unknown) {
        joinError.value = multiplayerErrorText(error)
        joining.value = false
    }
}

async function refresh() {
    if (loading.value) return
    loading.value = true
    failed.value = false
    timedOut.value = false
    try {
        lobbies.value = await fetchPublicLobbies(undefined, request.signal)
    } catch (error) {
        failed.value = true
        timedOut.value = error instanceof Error && /timed out/i.test(error.message)
    } finally {
        loaded.value = true
        loading.value = false
    }
}

function playerCount(lobby: PublicLobby) {
    if (lobby.maxPlayers === null) {
        return formatMessage(messages.playerCount, { count: lobby.playerCount })
    }
    return formatMessage(messages.playerCapacity, {
        count: lobby.playerCount,
        max: lobby.maxPlayers,
    })
}

onMounted(() => void refresh())
onUnmounted(() => request.abort())
</script>

<template>
    <section class="flex min-h-0 flex-1 flex-col gap-4" aria-labelledby="public-lobbies-title">
        <header class="flex flex-wrap items-start justify-between gap-3">
            <div class="min-w-0">
                <h2
                    id="public-lobbies-title"
                    class="m-0 text-xl font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(messages.title) }}
                </h2>
                <p class="mb-0 mt-1 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.description) }}
                </p>
            </div>
            <Button :icon="RefreshCwIcon" :disabled="loading" @click="refresh">
                {{ formatMessage(messages.refresh) }}
            </Button>
        </header>

        <Admonition type="info">
            {{ formatMessage(messages.joinHint) }}
        </Admonition>

        <form
            v-if="selectedLobby"
            class="flex min-w-0 flex-col gap-3 rounded-md border border-brand bg-surface-2 p-4"
            @submit.prevent="joinSelected"
        >
            <div class="flex min-w-0 items-start justify-between gap-3">
                <div class="min-w-0">
                    <h3 class="m-0 break-words text-base font-semibold">
                        {{ formatMessage(messages.join) }}: {{ selectedLobby.lobbyName }}
                    </h3>
                    <p class="mb-0 mt-1 text-xs text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.portHint) }}
                    </p>
                </div>
                <Button type="quiet" :disabled="joining" @click="closeJoin">
                    {{ formatMessage(messages.cancel) }}
                </Button>
            </div>
            <div class="grid min-w-0 gap-3 sm:grid-cols-3">
                <label class="flex min-w-0 flex-col gap-1 text-sm">
                    <span>{{ formatMessage(messages.playerName) }}</span>
                    <input
                        v-model="playerName"
                        class="min-w-0 rounded-md border border-divider bg-surface-1 px-3 py-2"
                        maxlength="64"
                        autocomplete="nickname"
                    />
                </label>
                <label class="flex min-w-0 flex-col gap-1 text-sm">
                    <span>{{ formatMessage(messages.gamePort) }}</span>
                    <input
                        v-model="gamePort"
                        class="min-w-0 rounded-md border border-divider bg-surface-1 px-3 py-2"
                        inputmode="numeric"
                        maxlength="5"
                    />
                </label>
                <label class="flex min-w-0 flex-col gap-1 text-sm">
                    <span>{{ formatMessage(messages.password) }}</span>
                    <input
                        v-model="password"
                        class="min-w-0 rounded-md border border-divider bg-surface-1 px-3 py-2"
                        type="password"
                        maxlength="128"
                        autocomplete="off"
                    />
                </label>
            </div>
            <p class="m-0 text-xs text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.portHint) }}
            </p>
            <p v-if="joinError" class="m-0 text-sm text-[var(--color-red)]" role="alert">
                {{ joinError }}
            </p>
            <div class="flex flex-wrap items-center gap-2">
                <Button type="colored" color="brand" :disabled="joining" native-type="submit">
                    <SpinnerIcon v-if="joining" class="animate-spin" />
                    <LogInIcon v-else />
                    {{
                        joining
                            ? formatMessage(messages.connecting, { name: selectedLobby.lobbyName })
                            : formatMessage(messages.join)
                    }}
                </Button>
            </div>
        </form>

        <div v-if="loading && !loaded" class="flex items-center gap-2 py-6 text-sm" role="status">
            <SpinnerIcon class="h-4 w-4 animate-spin" />
            <span>{{ formatMessage(messages.loading) }}</span>
        </div>
        <p v-else-if="failed" class="m-0 py-6 text-sm text-[var(--color-red)]" role="alert">
            {{ formatMessage(timedOut ? messages.timeout : messages.error) }}
        </p>
        <div
            v-else-if="loaded && lobbies.length === 0"
            class="flex flex-wrap items-center justify-between gap-3 py-6"
            role="status"
        >
            <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.empty) }}
            </p>
            <Button type="outlined" @click="router.push('/multiplayer/rooms')">
                <UsersIcon />
                {{ formatMessage(messages.createRoom) }}
            </Button>
        </div>
        <ul v-else class="m-0 grid list-none gap-2 p-0 sm:grid-cols-2">
            <li
                v-for="(lobby, index) in lobbies"
                :key="`${lobby.lobbyName}:${lobby.hostName}:${index}`"
                class="min-w-0 rounded-md border border-divider bg-surface-1 p-4"
            >
                <article class="flex min-w-0 flex-col gap-2">
                    <div class="flex min-w-0 items-start justify-between gap-3">
                        <h3
                            class="m-0 min-w-0 break-words text-base font-semibold text-[var(--color-text-primary)]"
                        >
                            {{ lobby.lobbyName }}
                        </h3>
                        <GlobeIcon class="h-4 w-4 shrink-0 text-[var(--color-text-tertiary)]" />
                    </div>
                    <p class="m-0 truncate text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.host, { name: lobby.hostName }) }}
                    </p>
                    <p v-if="lobby.description" class="m-0 break-words text-sm">
                        {{ lobby.description }}
                    </p>
                    <div
                        class="flex items-center gap-1.5 text-xs text-[var(--color-text-tertiary)]"
                    >
                        <UsersIcon class="h-3.5 w-3.5" />
                        <span>{{ playerCount(lobby) }}</span>
                    </div>
                    <Button
                        type="outlined"
                        color="brand"
                        :disabled="
                            lobby.maxPlayers !== null && lobby.playerCount >= lobby.maxPlayers
                        "
                        @click="openJoin(lobby)"
                    >
                        <LogInIcon />
                        {{ formatMessage(messages.join) }}
                    </Button>
                </article>
            </li>
        </ul>
    </section>
</template>
