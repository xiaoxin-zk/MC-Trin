<script setup lang="ts">
import { ExternalIcon } from '@modrinth/assets'
import { Button, defineMessages, NewModal, useVIntl } from '@modrinth/ui'
import { openUrl } from '@tauri-apps/plugin-opener'
import { computed, onUnmounted, ref } from 'vue'

import { begin_device_login, poll_device_login } from '@/helpers/auth'
import {
    createMinecraftDeviceLogin,
    type MinecraftDeviceLoginState,
    minecraftLoginErrorMessage,
} from '@/helpers/minecraft-device-login'

type MinecraftCredential = {
    account_id: string
    account_type: 'microsoft'
    profile: {
        id: string
        name: string
    }
}

const emit = defineEmits<{
    complete: [credentials: MinecraftCredential]
}>()

const { formatMessage } = useVIntl()
const modal = ref<InstanceType<typeof NewModal> | null>(null)
const loginState = ref<MinecraftDeviceLoginState>({ busy: false, flow: null, error: null })
const verificationError = ref<string | null>(null)
const busy = computed(() => loginState.value.busy)
const deviceFlow = computed(() => loginState.value.flow)
const deviceError = computed(() => loginState.value.error)

const messages = defineMessages({
    title: { id: 'minecraft-login.title', defaultMessage: 'Sign in to Minecraft' },
    deviceStarting: {
        id: 'minecraft-login.device-starting',
        defaultMessage: 'Requesting a device code...',
    },
    deviceDescription: {
        id: 'minecraft-login.device-description',
        defaultMessage: 'Open the page below on any device, then enter this code.',
    },
    deviceExpired: {
        id: 'minecraft-login.device-expired',
        defaultMessage: 'This device code expired. Start again to receive a new code.',
    },
    openVerification: {
        id: 'minecraft-login.open-verification',
        defaultMessage: 'Open verification page',
    },
    retry: {
        id: 'minecraft-login.device-retry',
        defaultMessage: 'Get a new code',
    },
})

const verificationUrl = computed(() => deviceFlow.value?.verification_uri ?? '')
const deviceLogin = createMinecraftDeviceLogin<MinecraftCredential>({
    begin: begin_device_login,
    poll: poll_device_login,
    change: (state) => {
        loginState.value = state
    },
    complete: (credentials) => {
        emit('complete', credentials)
        hide()
    },
    expiredMessage: () => formatMessage(messages.deviceExpired),
})

function resetDeviceLogin() {
    deviceLogin.stop()
    verificationError.value = null
}

function hide() {
    resetDeviceLogin()
    modal.value?.hide()
}

function showDeviceLogin() {
    resetDeviceLogin()
    modal.value?.show()
    void deviceLogin.start()
}

async function openDeviceVerification() {
    if (!verificationUrl.value) return
    verificationError.value = null
    try {
        await openUrl(verificationUrl.value)
    } catch (error) {
        verificationError.value = minecraftLoginErrorMessage(error)
    }
}

onUnmounted(resetDeviceLogin)

defineExpose({ showDeviceLogin, hide })
</script>

<template>
    <NewModal ref="modal" :header="formatMessage(messages.title)" :on-hide="resetDeviceLogin">
        <div class="flex min-w-[24rem] flex-col gap-4">
            <template v-if="deviceFlow">
                <p class="m-0 text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.deviceDescription) }}
                </p>
                <Button @click="openDeviceVerification"
                    ><ExternalIcon /> {{ formatMessage(messages.openVerification) }}
                </Button>
                <code
                    class="rounded-xl bg-surface-3 px-4 py-3 text-center text-xl font-bold tracking-[0.18em] text-[var(--color-text-primary)]"
                >
                    {{ deviceFlow.user_code }}
                </code>
                <p v-if="deviceError" class="m-0 text-sm text-red">{{ deviceError }}</p>
                <p v-if="verificationError" class="m-0 text-sm text-red">{{ verificationError }}</p>
            </template>
            <template v-else>
                <p v-if="busy" class="m-0 text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.deviceStarting) }}
                </p>
                <p v-if="deviceError" class="m-0 text-sm text-red">{{ deviceError }}</p>
            </template>
            <Button v-if="deviceError" :disabled="busy" @click="deviceLogin.start()">
                {{ formatMessage(messages.retry) }}
            </Button>
        </div>
    </NewModal>
</template>
