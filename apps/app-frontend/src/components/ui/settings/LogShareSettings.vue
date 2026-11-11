<script setup lang="ts">
import {
    Button,
    Combobox,
    defineMessages,
    injectNotificationManager,
    Toggle,
    useVIntl,
} from '@modrinth/ui'
import { computed, inject, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import LogAgentSourcePicker from '@/components/ui/LogAgentSourcePicker.vue'
import { type AIState, getAIState, sharedAIState } from '@/helpers/ai'
import {
    get_crash_analysis_ai_settings,
    get_log_share_settings,
    update_crash_analysis_ai_settings,
    update_log_share_settings,
} from '@/helpers/logs'
import type { AnalysisSettings } from '@/logagent/types'

import SettingsRow from './SettingsRow.vue'
import SettingsSection from './SettingsSection.vue'

const props = defineProps<{ localSetup?: boolean }>()
const emit = defineEmits<{ openProviders: []; savingChange: [saving: boolean] }>()

type LogShareSettings = {
    share_provider: 'logshare' | 'mclogs'
    ai_source: string
    auto_upload: boolean
    multi_file: boolean
    no_storage: boolean
    show_progress: boolean
    remote_consent: boolean
    /**
     * Explicit consent change (`grant` / `revoke`). Only set by a deliberate
     * user action; every other save omits it, so saving the page can never
     * change upload permission.
     */
    consent_action?: 'grant' | 'revoke'
}

const { formatMessage } = useVIntl()
const router = useRouter()
const replayOnboarding = inject<(mode: 'logagent') => Promise<void>>('replayOnboarding')
const { handleError } = injectNotificationManager()
const settings = ref<LogShareSettings>({
    share_provider: 'logshare',
    ai_source: 'custom',
    auto_upload: false,
    multi_file: true,
    no_storage: false,
    show_progress: true,
    remote_consent: false,
})
const aiSettings = ref<AnalysisSettings>({
    enabled: false,
    provider_id: '',
    model_id: '',
    ai_source: 'custom',
})
const loading = ref(true)
const saving = ref(false)

const messages = defineMessages({
    localSetupTitle: {
        id: 'app.logagent.settings.local-setup-title',
        defaultMessage: 'Local LogAgent',
    },
    localSetupDescription: {
        id: 'app.logagent.settings.local-setup-description',
        defaultMessage:
            'Configure your model for local analysis. Your default analysis source stays the same.',
    },
    analysisTitle: { id: 'app.logagent.settings.analysis-title', defaultMessage: 'Crash analysis' },
    analysisDescription: {
        id: 'app.logagent.settings.analysis-description',
        defaultMessage: 'Choose the default. You can change it for an individual analysis.',
    },
    sharingTitle: {
        id: 'app.logagent.settings.sharing-title',
        defaultMessage: 'Diagnostic sharing',
    },
    sharingDescription: {
        id: 'app.logagent.settings.sharing-description',
        defaultMessage: 'Used when you choose to share a diagnostic link.',
    },
    displayTitle: { id: 'app.logagent.settings.display-title', defaultMessage: 'Analysis display' },
    modelHelp: { id: 'app.logagent.settings.model-help', defaultMessage: 'Model requirements' },
    manageModels: {
        id: 'app.logagent.settings.manage-models',
        defaultMessage: 'Configure AI providers',
    },
    tour: { id: 'app.logagent.settings.tour', defaultMessage: 'About analysis sources' },
    supportedModels: {
        id: 'app.logagent.settings.supported-models',
        defaultMessage:
            'Choose a model that supports tool calling. Protocol support does not guarantee that every model accepts tools. Copilot Auto requires selecting an explicit model.',
    },
    shareProvider: {
        id: 'app.log-share.settings.share-provider',
        defaultMessage: 'Log sharing service',
    },
    enabled: {
        id: 'app.crash-analysis.ai.settings.enabled',
        defaultMessage: 'Enable local LogAgent',
    },
    enabledDescription: {
        id: 'app.crash-analysis.ai.settings.enabled-description',
        defaultMessage: 'Local rule-based crash analysis remains available without AI.',
    },
    provider: { id: 'app.crash-analysis.ai.settings.provider', defaultMessage: 'AI provider' },
    model: { id: 'app.crash-analysis.ai.settings.model', defaultMessage: 'Text model' },
    noProviders: {
        id: 'app.crash-analysis.ai.settings.no-providers',
        defaultMessage:
            'Enable an AI provider and a tool-capable model in Settings > AI before configuring local LogAgent.',
    },
    autoUpload: {
        id: 'app.log-share.settings.auto-upload',
        defaultMessage: 'Upload to LogShare automatically after a crash',
    },
    autoUploadDescription: {
        id: 'app.log-share.settings.auto-upload-description',
        defaultMessage:
            'Only controls automatic uploads after a crash. It does not grant or revoke permission to send logs to LogShare — use the permission row below for that. Your Minecraft token and computer username are removed, but server addresses, chat and mod names are not.',
    },
    remoteConsent: {
        id: 'app.log-share.settings.remote-consent',
        defaultMessage: 'Allow LogShare uploads',
    },
    remoteConsentGranted: {
        id: 'app.log-share.settings.remote-consent-granted',
        defaultMessage:
            'LogShare uploads are allowed: crash-log content may be sent to api.logshare.cn. Revoke this to stop LogShare uploads; a separately configured AI provider may still receive selected excerpts.',
    },
    remoteConsentRevoked: {
        id: 'app.log-share.settings.remote-consent-revoked',
        defaultMessage:
            'LogShare permission is off. No logs are sent to LogShare until you allow it; a separately configured AI provider may still receive selected excerpts.',
    },
    revokeConsent: {
        id: 'app.log-share.settings.revoke-consent',
        defaultMessage: 'Revoke LogShare permission',
    },
    grantConsent: {
        id: 'app.log-share.settings.grant-consent',
        defaultMessage: 'Allow LogShare uploads',
    },
    multiFile: { id: 'app.log-share.settings.multi-file', defaultMessage: 'Multi-file upload' },
    multiFileDescription: {
        id: 'app.log-share.settings.multi-file-description',
        defaultMessage: 'Upload latest.log, crash reports, hs_err and launcher logs together.',
    },
    noStorage: {
        id: 'app.log-share.settings.no-storage',
        defaultMessage: 'Ask LogShare not to store the logs',
    },
    noStorageDescription: {
        id: 'app.log-share.settings.no-storage-description',
        defaultMessage:
            'The full logs are still uploaded to api.logshare.cn. This only requests that LogShare not keep a stored copy; the launcher cannot verify what the service retains. Disables the automatic summary.',
    },
    showProgress: {
        id: 'app.log-share.settings.show-progress',
        defaultMessage: 'Show AI analysis progress',
    },
    showProgressDescription: {
        id: 'app.log-share.settings.show-progress-description',
        defaultMessage:
            'Show readable investigation steps and keep their history after analysis. Diagnostic text appears as it is generated when the provider supports streaming.',
    },
})

const emptyState: AIState = { settings: { enabled: false }, catalog_source: '', providers: [] }
const aiState = computed(() => sharedAIState.value ?? emptyState)
const providers = computed(() =>
    aiState.value.providers.filter(
        (provider) => provider.enabled && provider.models.some((model) => model.enabled),
    ),
)
const providerOptions = computed(() =>
    providers.value.map((provider) => ({
        value: provider.provider_id,
        label: provider.custom_name || provider.provider_id,
    })),
)
const modelOptions = computed(
    () =>
        providers.value
            .find((provider) => provider.provider_id === aiSettings.value.provider_id)
            ?.models.filter((model) => model.enabled)
            .map((model) => ({ value: model.id, label: model.name || model.id })) ?? [],
)

const isCustom = computed(() => aiSettings.value.ai_source === 'custom')

const shareProviderOptions = [
    { value: 'logshare', label: 'LogShare' },
    { value: 'mclogs', label: 'mclo.gs' },
]

async function saveShare(next: LogShareSettings): Promise<void> {
    if (saving.value) return
    saving.value = true
    emit('savingChange', true)
    try {
        await update_log_share_settings(next)
        settings.value = next
    } catch (error) {
        handleError(error)
    } finally {
        saving.value = false
        emit('savingChange', false)
    }
}

async function saveAI(next: AnalysisSettings): Promise<void> {
    if (saving.value) return
    saving.value = true
    emit('savingChange', true)
    try {
        await update_crash_analysis_ai_settings(next)
        aiSettings.value = next
        settings.value = { ...settings.value, ai_source: next.ai_source }
    } catch (error) {
        handleError(error)
    } finally {
        saving.value = false
        emit('savingChange', false)
    }
}

/** Grants or revokes LogShare permission, independently of auto-upload. */
async function setRemoteConsent(enabled: boolean): Promise<void> {
    if (saving.value) return
    saving.value = true
    emit('savingChange', true)
    const { consent_action: _ignored, ...rest } = settings.value
    const next: LogShareSettings = {
        ...rest,
        // Revoking also returns the stored preference to local, mirroring the
        // backend, so the toggle cannot keep showing an upload that will be
        // refused.
        auto_upload: enabled ? rest.auto_upload : false,
        consent_action: enabled ? 'grant' : 'revoke',
    }
    try {
        await update_log_share_settings(next)
        const refreshed = await get_log_share_settings()
        settings.value = refreshed
        aiSettings.value = await get_crash_analysis_ai_settings()
    } catch (error) {
        handleError(error)
    } finally {
        saving.value = false
        emit('savingChange', false)
    }
}

function updateShare(patch: Partial<LogShareSettings>): void {
    // `consent_action` is never carried over from the previous state: only a
    // deliberate switch below supplies one.
    const { consent_action: _ignored, ...rest } = settings.value
    void saveShare({ ...rest, ...patch })
}

function selectShareProvider(value: unknown): void {
    const provider = value === 'mclogs' ? 'mclogs' : 'logshare'
    updateShare({ share_provider: provider, ai_source: aiSettings.value.ai_source })
}

function selectAiSource(value: unknown): void {
    const source = value === 'custom' ? 'custom' : 'logshare'
    if (source === aiSettings.value.ai_source) return
    void saveAI({ ...aiSettings.value, ai_source: source })
}

function updateEnabled(enabled: boolean): void {
    void saveAI({ ...aiSettings.value, enabled })
}

function updateProvider(providerId: string): void {
    const provider = providers.value.find((item) => item.provider_id === providerId)
    const modelId = provider?.models.find((model) => model.enabled)?.id ?? ''
    void saveAI({ ...aiSettings.value, provider_id: providerId, model_id: modelId })
}

function updateModel(modelId: string): void {
    void saveAI({ ...aiSettings.value, model_id: modelId })
}

function openProviders(): void {
    if (props.localSetup) emit('openProviders')
    else void router.push('/settings#ai')
}

onMounted(async () => {
    try {
        const [nextShare, nextAI] = await Promise.all([
            get_log_share_settings(),
            get_crash_analysis_ai_settings(),
            getAIState().catch(handleError),
        ])
        settings.value = { ...nextShare, ai_source: nextAI.ai_source }
        aiSettings.value = nextAI
    } catch (error) {
        handleError(error)
    } finally {
        loading.value = false
    }
})
</script>

<template>
    <div v-if="!loading" class="flex min-w-0 flex-col gap-6">
        <SettingsSection
            :title="formatMessage(localSetup ? messages.localSetupTitle : messages.analysisTitle)"
            :description="
                formatMessage(
                    localSetup ? messages.localSetupDescription : messages.analysisDescription,
                )
            "
        >
            <template #extra
                ><Button v-if="!localSetup" type="quiet" @click="replayOnboarding?.('logagent')">{{
                    formatMessage(messages.tour)
                }}</Button></template
            >
            <div
                v-if="!localSetup"
                id="settings-target-log-share"
                data-onboarding-id="logagent-settings-source"
                class="p-4"
            >
                <LogAgentSourcePicker
                    :model-value="aiSettings.ai_source"
                    :disabled="saving"
                    @update:model-value="selectAiSource"
                />
            </div>
            <template v-if="isCustom || localSetup">
                <SettingsRow inline-control>
                    <template #label>{{ formatMessage(messages.enabled) }}</template>
                    <template #description>{{
                        formatMessage(messages.enabledDescription)
                    }}</template>
                    <template #control
                        ><Toggle
                            id="crash-analysis-ai-enabled"
                            :model-value="aiSettings.enabled"
                            :disabled="saving || !aiState.settings.enabled || !providers.length"
                            @update:model-value="updateEnabled(!!$event)"
                    /></template>
                </SettingsRow>
                <template v-if="providers.length">
                    <SettingsRow>
                        <template #label>{{ formatMessage(messages.provider) }}</template>
                        <template #control
                            ><Combobox
                                id="crash-analysis-ai-provider"
                                :model-value="aiSettings.provider_id"
                                :options="providerOptions"
                                :disabled="saving"
                                @update:model-value="updateProvider(String($event))"
                        /></template>
                    </SettingsRow>
                    <SettingsRow>
                        <template #label>{{ formatMessage(messages.model) }}</template>
                        <template #control
                            ><Combobox
                                id="crash-analysis-ai-model"
                                :model-value="aiSettings.model_id"
                                :options="modelOptions"
                                :disabled="saving || !aiSettings.provider_id"
                                @update:model-value="updateModel(String($event))"
                        /></template>
                    </SettingsRow>
                    <details class="px-4 py-3 text-sm text-[var(--color-text-tertiary)]">
                        <summary class="cursor-pointer">
                            {{ formatMessage(messages.modelHelp) }}
                        </summary>
                        <p class="m-0 mt-2 leading-relaxed">
                            {{ formatMessage(messages.supportedModels) }}
                        </p>
                    </details>
                </template>
                <div
                    v-if="!providers.length || !aiState.settings.enabled"
                    class="flex flex-col items-start gap-2 px-4 py-3"
                >
                    <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.noProviders) }}
                    </p>
                    <Button type="outlined" @click="openProviders">{{
                        formatMessage(messages.manageModels)
                    }}</Button>
                </div>
            </template>
            <div v-else data-onboarding-id="logagent-settings-cloud">
                <SettingsRow inline-control>
                    <template #label>{{ formatMessage(messages.noStorage) }}</template>
                    <template #description>{{
                        formatMessage(messages.noStorageDescription)
                    }}</template>
                    <template #control
                        ><Toggle
                            id="log-share-no-storage"
                            :model-value="settings.no_storage"
                            :disabled="saving"
                            @update:model-value="updateShare({ no_storage: !!$event })"
                    /></template>
                </SettingsRow>
                <SettingsRow inline-control>
                    <template #label>{{ formatMessage(messages.autoUpload) }}</template>
                    <template #description>{{
                        formatMessage(messages.autoUploadDescription)
                    }}</template>
                    <template #control
                        ><Toggle
                            id="log-share-auto-upload"
                            :model-value="settings.auto_upload"
                            :disabled="saving || settings.no_storage || !settings.remote_consent"
                            @update:model-value="updateShare({ auto_upload: !!$event })"
                    /></template>
                </SettingsRow>
            </div>
            <!-- Kept outside the source branch: permission must stay revocable
                 even when the selected source is local. -->
            <div v-if="!localSetup" data-onboarding-id="logagent-settings-consent">
                <SettingsRow>
                    <template #label>{{ formatMessage(messages.remoteConsent) }}</template>
                    <template #description>
                        {{
                            formatMessage(
                                settings.remote_consent
                                    ? messages.remoteConsentGranted
                                    : messages.remoteConsentRevoked,
                            )
                        }}
                    </template>
                    <template #control>
                        <Button
                            :id="
                                settings.remote_consent
                                    ? 'log-share-revoke-consent'
                                    : 'log-share-grant-consent'
                            "
                            type="outlined"
                            :disabled="saving"
                            @click="setRemoteConsent(!settings.remote_consent)"
                        >
                            {{
                                formatMessage(
                                    settings.remote_consent
                                        ? messages.revokeConsent
                                        : messages.grantConsent,
                                )
                            }}
                        </Button>
                    </template>
                </SettingsRow>
            </div>
        </SettingsSection>
        <SettingsSection
            v-if="!localSetup"
            data-onboarding-id="logagent-settings-sharing"
            :title="formatMessage(messages.sharingTitle)"
            :description="formatMessage(messages.sharingDescription)"
        >
            <SettingsRow>
                <template #label>{{ formatMessage(messages.shareProvider) }}</template>
                <template #control
                    ><Combobox
                        id="log-share-provider"
                        :model-value="settings.share_provider"
                        :options="shareProviderOptions"
                        :disabled="saving"
                        @update:model-value="selectShareProvider"
                /></template>
            </SettingsRow>
            <SettingsRow inline-control>
                <template #label>{{ formatMessage(messages.multiFile) }}</template>
                <template #description>{{ formatMessage(messages.multiFileDescription) }}</template>
                <template #control
                    ><Toggle
                        id="log-share-multi-file"
                        :model-value="settings.multi_file"
                        :disabled="saving"
                        @update:model-value="updateShare({ multi_file: !!$event })"
                /></template>
            </SettingsRow>
        </SettingsSection>
        <SettingsSection v-if="!localSetup" :title="formatMessage(messages.displayTitle)">
            <SettingsRow inline-control>
                <template #label>{{ formatMessage(messages.showProgress) }}</template>
                <template #description>{{
                    formatMessage(messages.showProgressDescription)
                }}</template>
                <template #control
                    ><Toggle
                        id="log-share-show-progress"
                        :model-value="settings.show_progress"
                        :disabled="saving"
                        @update:model-value="updateShare({ show_progress: !!$event })"
                /></template>
            </SettingsRow>
        </SettingsSection>
    </div>
</template>
