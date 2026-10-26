<script setup lang="ts">
import { defineMessages, injectNotificationManager, Toggle, useVIntl } from '@modrinth/ui'
import { computed, ref } from 'vue'

import PrivilegedConsentModal from '@/components/ui/modal/PrivilegedConsentModal.vue'
import {
    get as getSettings,
    getPrivacySettings,
    setDiscordRpcEnabled,
    update as updateSettings,
} from '@/helpers/settings'

import SettingsRow from './SettingsRow.vue'
import SettingsSaveStatus from './SettingsSaveStatus.vue'
import SettingsSection from './SettingsSection.vue'

const { formatMessage } = useVIntl()
const { handleError } = injectNotificationManager()
const privacy = ref(await getPrivacySettings())
const discordSaving = ref(false)
const schemeSaving = ref(false)
const allowExternalScheme = ref((await getSettings()).allow_external_scheme)
const privilegedSaving = ref(false)
const allowPrivilegedScheme = ref((await getSettings()).allow_privileged_scheme)
const privilegedConsentModal = ref<InstanceType<typeof PrivilegedConsentModal> | null>(null)
const lastSaveState = ref<'idle' | 'saved' | 'error'>('idle')
const retrySave = ref<(() => void) | undefined>()

const messages = defineMessages({
    privacyTitle: { id: 'app.settings.privacy.section-title', defaultMessage: 'Privacy' },
    securityTitle: {
        id: 'app.settings.privacy.security-title',
        defaultMessage: 'Security',
    },
    discordRpc: {
        id: 'app.settings.privacy.discord-rpc',
        defaultMessage: 'Discord Rich Presence',
    },
    discordRpcDescription: {
        id: 'app.settings.privacy.discord-rpc-description',
        defaultMessage: 'Show your current launcher or game activity in Discord.',
    },
    externalScheme: {
        id: 'app.settings.privacy.external-scheme',
        defaultMessage: 'Allow external links',
    },
    externalSchemeDescription: {
        id: 'app.settings.privacy.external-scheme-description',
        defaultMessage:
            'Respond to mctrin:// links from browsers and other apps. Turning this off blocks launches, installs and page jumps from outside.',
    },
    privilegedScheme: {
        id: 'app.settings.privacy.privileged-scheme',
        defaultMessage: 'Allow privileged link actions',
    },
    privilegedSchemeDescription: {
        id: 'app.settings.privacy.privileged-scheme-description',
        defaultMessage:
            'Let mctrin:// links change settings and stop game processes. Turning this on requires reading a risk warning; every action still asks for your confirmation.',
    },
})
const saveStatus = computed(() => {
    if (discordSaving.value || schemeSaving.value || privilegedSaving.value) return 'saving'
    return lastSaveState.value
})

async function updateExternalScheme(value: boolean) {
    if (schemeSaving.value) return
    const previous = allowExternalScheme.value
    allowExternalScheme.value = value
    schemeSaving.value = true
    lastSaveState.value = 'idle'
    retrySave.value = undefined
    try {
        await updateSettings({ allow_external_scheme: value })
        lastSaveState.value = 'saved'
    } catch (error) {
        allowExternalScheme.value = previous
        retrySave.value = () => void updateExternalScheme(value)
        lastSaveState.value = 'error'
        handleError(error)
    } finally {
        schemeSaving.value = false
    }
}

async function updatePrivilegedScheme(value: boolean) {
    if (privilegedSaving.value) return
    // 启用前强制阅读确认；取消则保持关闭
    if (value) {
        const consented = await privilegedConsentModal.value?.request()
        if (!consented) return
    }
    const previous = allowPrivilegedScheme.value
    allowPrivilegedScheme.value = value
    privilegedSaving.value = true
    lastSaveState.value = 'idle'
    retrySave.value = undefined
    try {
        await updateSettings({ allow_privileged_scheme: value })
        lastSaveState.value = 'saved'
    } catch (error) {
        allowPrivilegedScheme.value = previous
        retrySave.value = () => void updatePrivilegedScheme(value)
        lastSaveState.value = 'error'
        handleError(error)
    } finally {
        privilegedSaving.value = false
    }
}

async function updateDiscordRpc(value: boolean) {
    if (discordSaving.value) return
    const previous = privacy.value.discord_rpc
    privacy.value.discord_rpc = value
    discordSaving.value = true
    lastSaveState.value = 'idle'
    retrySave.value = undefined
    try {
        const saved = await setDiscordRpcEnabled(value)
        privacy.value.discord_rpc = saved.discord_rpc
        lastSaveState.value = 'saved'
    } catch (error) {
        privacy.value.discord_rpc = previous
        retrySave.value = () => void updateDiscordRpc(value)
        lastSaveState.value = 'error'
        handleError(error)
    } finally {
        discordSaving.value = false
    }
}
</script>

<template>
    <div class="flex w-full flex-col gap-6">
        <SettingsSection
            :title="formatMessage(messages.privacyTitle)"
            title-id="settings-target-privacy"
        >
            <template #extra>
                <SettingsSaveStatus :status="saveStatus" :retry="retrySave" />
            </template>
            <SettingsRow>
                <template #label>
                    <span id="settings-target-privacy-discord-rpc" tabindex="-1">
                        {{ formatMessage(messages.discordRpc) }}
                    </span>
                </template>
                <template #description>{{
                    formatMessage(messages.discordRpcDescription)
                }}</template>
                <template #control>
                    <Toggle
                        id="privacy-discord-rpc"
                        :model-value="privacy.discord_rpc"
                        :disabled="discordSaving"
                        @update:model-value="(value) => updateDiscordRpc(!!value)"
                    />
                </template>
            </SettingsRow>
        </SettingsSection>
        <SettingsSection
            :title="formatMessage(messages.securityTitle)"
            title-id="settings-target-privacy"
        >
            <template #extra>
                <SettingsSaveStatus :status="saveStatus" :retry="retrySave" />
            </template>
            <SettingsRow>
                <template #label>
                    <span id="settings-target-privacy-external-scheme" tabindex="-1">
                        {{ formatMessage(messages.externalScheme) }}
                    </span>
                </template>
                <template #description>{{
                    formatMessage(messages.externalSchemeDescription)
                }}</template>
                <template #control>
                    <Toggle
                        id="privacy-external-scheme"
                        :model-value="allowExternalScheme"
                        :disabled="schemeSaving"
                        @update:model-value="(value) => updateExternalScheme(!!value)"
                    />
                </template>
            </SettingsRow>
            <SettingsRow>
                <template #label>
                    <span id="settings-target-privacy-privileged-scheme" tabindex="-1">
                        {{ formatMessage(messages.privilegedScheme) }}
                    </span>
                </template>
                <template #description>{{
                    formatMessage(messages.privilegedSchemeDescription)
                }}</template>
                <template #control>
                    <Toggle
                        id="privacy-privileged-scheme"
                        :model-value="allowPrivilegedScheme"
                        :disabled="privilegedSaving"
                        @update:model-value="(value) => updatePrivilegedScheme(!!value)"
                    />
                </template>
            </SettingsRow>
        </SettingsSection>
    </div>
    <PrivilegedConsentModal ref="privilegedConsentModal" />
</template>
