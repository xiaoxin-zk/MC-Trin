<script setup lang="ts">
import { ExternalIcon, RocketIcon } from '@modrinth/assets'
import {
    Admonition,
    Button,
    Card,
    CopyCode,
    defineMessages,
    StyledInput,
    useVIntl,
} from '@modrinth/ui'
import { openUrl } from '@tauri-apps/plugin-opener'
import { ref } from 'vue'

import { multiplayer, validLocalPort } from '@/helpers/multiplayer'

const { formatMessage } = useVIntl()
const messages = defineMessages({
    title: { id: 'app.multiplayer.sakura.title', defaultMessage: 'SakuraFrp companion' },
    description: {
        id: 'app.multiplayer.sakura.description',
        defaultMessage:
            'Use the official SakuraFrp client to expose your Minecraft LAN port. MC-Trin does not bundle the third-party client or handle your account.',
    },
    localPort: {
        id: 'app.multiplayer.sakura.local-port',
        defaultMessage: 'Minecraft LAN port',
    },
    localPortHint: {
        id: 'app.multiplayer.sakura.local-port-hint',
        defaultMessage: 'Open your world to LAN first, then enter the port Minecraft shows.',
    },
    publicAddress: {
        id: 'app.multiplayer.sakura.public-address',
        defaultMessage: 'SakuraFrp public address',
    },
    publicAddressHint: {
        id: 'app.multiplayer.sakura.public-address-hint',
        defaultMessage: 'Paste the address shown by SakuraFrp after you start the tunnel.',
    },
    launch: { id: 'app.multiplayer.sakura.launch', defaultMessage: 'Open SakuraFrp client' },
    website: { id: 'app.multiplayer.sakura.website', defaultMessage: 'Open official website' },
    installed: {
        id: 'app.multiplayer.sakura.installed',
        defaultMessage: 'The installed SakuraFrp client was opened.',
    },
    notInstalled: {
        id: 'app.multiplayer.sakura.not-installed',
        defaultMessage: 'SakuraFrp was not found in its standard Windows install location.',
    },
    steps: {
        id: 'app.multiplayer.sakura.steps',
        defaultMessage:
            'Create or select a TCP tunnel in SakuraFrp, map it to the local port above, then share the public address with friends.',
    },
    invalidPort: {
        id: 'app.multiplayer.sakura.invalid-port',
        defaultMessage: 'Minecraft LAN port must be between 1 and 65535.',
    },
})

const localPort = ref('25565')
const publicAddress = ref('')
const opening = ref(false)
const status = ref<'idle' | 'opened' | 'missing'>('idle')
const error = ref('')

async function openClient() {
    if (!validLocalPort(localPort.value)) {
        error.value = formatMessage(messages.invalidPort)
        return
    }
    opening.value = true
    error.value = ''
    try {
        const result = await multiplayer.openSakuraClient()
        status.value = result.installed ? 'opened' : 'missing'
    } catch (cause) {
        error.value = cause instanceof Error ? cause.message : String(cause)
    } finally {
        opening.value = false
    }
}

function openWebsite() {
    void openUrl('https://www.natfrp.com/user/')
}
</script>

<template>
    <Card class="!m-0">
        <div class="flex min-w-0 flex-col gap-5">
            <div class="flex items-start gap-3">
                <div
                    class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-brand-highlight text-brand"
                >
                    <RocketIcon class="size-5" />
                </div>
                <div class="min-w-0">
                    <h2 class="m-0 text-lg font-semibold text-[var(--color-text-primary)]">
                        {{ formatMessage(messages.title) }}
                    </h2>
                    <p class="mb-0 mt-1 text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.description) }}
                    </p>
                </div>
            </div>

            <Admonition type="info">
                {{ formatMessage(messages.steps) }}
            </Admonition>

            <div class="grid min-w-0 gap-4 md:grid-cols-2">
                <label class="flex min-w-0 flex-col gap-2 text-sm">
                    <span class="font-semibold text-[var(--color-text-primary)]">
                        {{ formatMessage(messages.localPort) }}
                    </span>
                    <StyledInput v-model="localPort" inputmode="numeric" placeholder="25565" />
                    <span class="text-xs text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.localPortHint) }}
                    </span>
                </label>
                <label class="flex min-w-0 flex-col gap-2 text-sm">
                    <span class="font-semibold text-[var(--color-text-primary)]">
                        {{ formatMessage(messages.publicAddress) }}
                    </span>
                    <StyledInput v-model="publicAddress" placeholder="example.natfrp.cloud:25565" />
                    <span class="text-xs text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.publicAddressHint) }}
                    </span>
                </label>
            </div>

            <CopyCode v-if="publicAddress.trim()" :text="publicAddress.trim()" />

            <p v-if="status === 'opened'" class="m-0 text-sm text-green" role="status">
                {{ formatMessage(messages.installed) }}
            </p>
            <p
                v-else-if="status === 'missing'"
                class="m-0 text-sm text-[var(--color-text-tertiary)]"
                role="status"
            >
                {{ formatMessage(messages.notInstalled) }}
            </p>
            <p v-if="error" class="m-0 text-sm text-[var(--color-red)]" role="alert">{{ error }}</p>

            <div class="flex flex-wrap gap-2">
                <Button type="colored" color="brand" :disabled="opening" @click="openClient">
                    <RocketIcon />
                    {{ formatMessage(messages.launch) }}
                </Button>
                <Button type="outlined" @click="openWebsite">
                    <ExternalIcon />
                    {{ formatMessage(messages.website) }}
                </Button>
            </div>
        </div>
    </Card>
</template>
