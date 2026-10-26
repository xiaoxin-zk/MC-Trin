<script setup lang="ts">
import { RefreshCwIcon } from '@modrinth/assets'
import { Button, defineMessages, injectNotificationManager, Toggle, useVIntl } from '@modrinth/ui'
import { getVersion } from '@tauri-apps/api/app'
import { ref } from 'vue'

import { getUpdateChannel, getUpdatePreferences, setUpdatePreferences } from '@/helpers/settings.ts'
import { type AppUpdateCheckResult, checkForAppUpdate } from '@/providers/app-update.ts'

import SettingsRow from './SettingsRow.vue'
import SettingsSection from './SettingsSection.vue'

const { formatMessage } = useVIntl()
const { handleError } = injectNotificationManager()
const [activeChannel, initialUpdatePreferences, currentVersion] = await Promise.all([
    getUpdateChannel(),
    getUpdatePreferences(),
    getVersion(),
])
const updatePreferences = ref(initialUpdatePreferences)
const checking = ref(false)
const checkResult = ref<AppUpdateCheckResult | 'failed' | null>(null)

const messages = defineMessages({
    preferencesTitle: {
        id: 'app.settings.updates.preferences.title',
        defaultMessage: 'Update behavior',
    },
    checkTitle: {
        id: 'app.settings.updates.check.title',
        defaultMessage: 'Check for updates',
    },
    immediateFetch: {
        id: 'app.settings.updates.immediate-fetch',
        defaultMessage: 'Get updates as soon as they are available',
    },
    immediateFetchDescription: {
        id: 'app.settings.updates.immediate-fetch-description',
        defaultMessage: 'Receive stable updates soon after they are released.',
    },
    immediateFetchAlwaysOn: {
        id: 'app.settings.updates.immediate-fetch.beta-description',
        defaultMessage: 'This version receives updates as soon as they are available.',
    },
    pause: {
        id: 'app.settings.updates.pause',
        defaultMessage: 'Pause updates',
    },
    pauseDescription: {
        id: 'app.settings.updates.pause-description',
        defaultMessage:
            'Stop automatic update checks, downloads, and update notifications until you resume updates.',
    },
    check: {
        id: 'app.settings.updates.check',
        defaultMessage: 'Check for updates',
    },
    checking: {
        id: 'app.settings.updates.checking',
        defaultMessage: 'Checking for updates…',
    },
    available: {
        id: 'app.settings.updates.available',
        defaultMessage: 'An update is available.',
    },
    upToDate: {
        id: 'app.settings.updates.up-to-date',
        defaultMessage: 'MC-Trin is up to date.',
    },
    disabled: {
        id: 'app.settings.updates.disabled',
        defaultMessage: 'Updates are disabled in this build.',
    },
    offline: {
        id: 'app.settings.updates.offline',
        defaultMessage: 'Connect to the internet to check for updates.',
    },
    failed: {
        id: 'app.settings.updates.failed',
        defaultMessage: 'Could not check for updates.',
    },
    paused: {
        id: 'app.settings.updates.paused',
        defaultMessage: 'Updates are paused.',
    },
    security: {
        id: 'app.settings.updates.security',
        defaultMessage: 'Updates are installed only when their cryptographic signature is valid.',
    },
    currentVersion: {
        id: 'app.settings.updates.current-version',
        defaultMessage: 'Current version {version}',
    },
})

const resultMessages: Record<AppUpdateCheckResult | 'failed', keyof typeof messages> = {
    available: 'available',
    'up-to-date': 'upToDate',
    disabled: 'disabled',
    offline: 'offline',
    failed: 'failed',
    paused: 'paused',
}

async function saveUpdatePreferences() {
    try {
        await setUpdatePreferences(updatePreferences.value)
    } catch (error) {
        handleError(error)
    }
}

async function checkForUpdates() {
    checking.value = true
    checkResult.value = null

    try {
        checkResult.value = await checkForAppUpdate()
    } catch (error) {
        checkResult.value = 'failed'
        handleError(error)
    } finally {
        checking.value = false
    }
}
</script>

<template>
    <div class="flex flex-col gap-6">
        <SettingsSection :title="formatMessage(messages.preferencesTitle)">
            <SettingsRow>
                <template #label>{{ formatMessage(messages.immediateFetch) }}</template>
                <template #description>
                    {{
                        formatMessage(
                            activeChannel === 'beta'
                                ? messages.immediateFetchAlwaysOn
                                : messages.immediateFetchDescription,
                        )
                    }}
                </template>
                <template #control>
                    <Toggle
                        id="immediate-update-fetch"
                        v-model="updatePreferences.immediateUpdateFetch"
                        :disabled="activeChannel === 'beta'"
                        @update:model-value="saveUpdatePreferences"
                    />
                </template>
            </SettingsRow>
            <SettingsRow>
                <template #label>{{ formatMessage(messages.pause) }}</template>
                <template #description>{{ formatMessage(messages.pauseDescription) }}</template>
                <template #control>
                    <Toggle
                        id="pause-updates"
                        v-model="updatePreferences.updatesPaused"
                        @update:model-value="saveUpdatePreferences"
                    />
                </template>
            </SettingsRow>
        </SettingsSection>

        <SettingsSection :title="formatMessage(messages.checkTitle)">
            <div class="update-check-panel">
                <div class="update-check-heading">
                    <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.currentVersion, { version: currentVersion }) }}
                    </p>
                    <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.security) }}
                    </p>
                </div>
                <Button type="colored" color="brand" :disabled="checking" @click="checkForUpdates">
                    <RefreshCwIcon :class="{ 'animate-spin': checking }" />
                    {{ formatMessage(checking ? messages.checking : messages.check) }}
                </Button>
                <p
                    v-if="checkResult"
                    class="update-check-result"
                    :class="`update-check-result-${checkResult}`"
                    role="status"
                >
                    {{ formatMessage(messages[resultMessages[checkResult]]) }}
                </p>
            </div>
        </SettingsSection>
    </div>
</template>

<style scoped>
.update-check-panel {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--gap-md);
    padding: var(--gap-lg);
}

.update-check-heading {
    display: flex;
    flex-direction: column;
    gap: var(--gap-xs);
}

.update-check-result {
    margin: 0;
    padding: var(--gap-sm) var(--gap-md);
    border: 1px solid var(--surface-4);
    border-radius: var(--radius-sm);
    background: var(--surface-1);
    color: var(--color-text-tertiary);
    font-size: 0.875rem;
    line-height: 1.4;
}

.update-check-result-available {
    border-color: color-mix(in srgb, var(--color-brand) 45%, var(--surface-4));
    color: var(--color-brand);
}

.update-check-result-failed,
.update-check-result-offline {
    border-color: color-mix(in srgb, var(--color-red) 45%, var(--surface-4));
    color: var(--color-red);
}

.update-check-result-paused,
.update-check-result-disabled {
    border-color: color-mix(in srgb, var(--color-yellow) 45%, var(--surface-4));
    color: var(--color-yellow);
}
</style>
