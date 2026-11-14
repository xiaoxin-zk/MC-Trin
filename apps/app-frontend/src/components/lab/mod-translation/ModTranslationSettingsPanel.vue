<script setup lang="ts">
import { Combobox, type ComboboxOption, defineMessages, Toggle, useVIntl } from '@modrinth/ui'
import { computed, ref } from 'vue'

import AIIcon from '@/components/ui/settings/AIIcon.vue'
import { type AIProviderDefinition, getAICatalog, getAIState, sharedAIState } from '@/helpers/ai'
import { getTranslationSettings } from '@/helpers/translation'
import type { ModTranslationOptions } from '@/lab/mod-translation/types.ts'

const props = defineProps<{
    modelValue: ModTranslationOptions
    providerId: string
    modelId: string
}>()

const emit = defineEmits<{
    'update:modelValue': [value: ModTranslationOptions]
    'update:providerId': [value: string]
    'update:modelId': [value: string]
}>()

const { formatMessage } = useVIntl()
const aiCatalog = ref<AIProviderDefinition[]>([])
const loading = ref(true)
const aiLoadFailed = ref(false)

const AI_LOAD_TIMEOUT_MS = 8000

function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
    return new Promise<T>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('timeout')), timeoutMs)
        promise.then(
            (value) => {
                clearTimeout(timer)
                resolve(value)
            },
            (error) => {
                clearTimeout(timer)
                reject(error)
            },
        )
    })
}

const messages = defineMessages({
    provider: { id: 'app.lab.mod-translation.provider', defaultMessage: 'AI provider' },
    model: { id: 'app.lab.mod-translation.model', defaultMessage: 'Text model' },
    aiNotConfigured: {
        id: 'app.lab.mod-translation.ai-not-configured',
        defaultMessage:
            'AI is not configured. Open the AI settings to enable a provider and model.',
    },
    aiLoadError: {
        id: 'app.lab.mod-translation.ai-load-error',
        defaultMessage: 'AI settings could not be loaded. Check your connection and try again.',
    },
    options: { id: 'app.lab.mod-translation.options', defaultMessage: 'Options' },
    batchSize: { id: 'app.lab.mod-translation.batch-size', defaultMessage: 'Batch size' },
    generateModName: {
        id: 'app.lab.mod-translation.generate-mod-name',
        defaultMessage: 'AI-generate a Chinese mod name',
    },
    repairEnabled: {
        id: 'app.lab.mod-translation.repair-enabled',
        defaultMessage: 'Repair difficult translations after verification',
    },
    classTextEnabled: {
        id: 'app.lab.mod-translation.class-text-enabled',
        defaultMessage: 'Rewrite advanced .class text candidates',
    },
    unavailableOption: {
        id: 'app.lab.mod-translation.unavailable-option',
        defaultMessage: 'Unavailable',
    },
    selectionUnavailable: {
        id: 'app.lab.mod-translation.selection-unavailable',
        defaultMessage:
            'The saved AI provider or model is unavailable right now. Pick another one to translate.',
    },
})

const configuredProviders = computed(() =>
    (sharedAIState.value?.providers ?? []).filter(
        (provider) => provider.enabled && provider.models.some((model) => model.enabled),
    ),
)
const aiAvailable = computed(
    () => !!sharedAIState.value?.settings.enabled && configuredProviders.value.length > 0,
)

function providerLabel(providerId: string) {
    const provider = (sharedAIState.value?.providers ?? []).find(
        (item) => item.provider_id === providerId,
    )
    return (
        provider?.custom_name ||
        aiCatalog.value.find((definition) => definition.id === providerId)?.name ||
        providerId
    )
}

/**
 * Display-only fallbacks. The first configured provider/model is shown as a
 * hint when nothing has been chosen yet, but it is never written to the page
 * store from here: only `selectProvider` and the model combobox emit, and only
 * on a user pick.
 */
const displayProviderId = computed(
    () => props.providerId || configuredProviders.value[0]?.provider_id || '',
)
const displayProviderLabel = computed(() =>
    props.providerId ? undefined : providerLabel(displayProviderId.value),
)
const displayModelId = computed(() => {
    if (props.modelId) return props.modelId
    // Only suggest the first model while nothing has been chosen at all; a
    // saved provider with no model must keep showing "no model" so the user
    // knows a pick is still required.
    if (props.providerId) return ''
    const provider = (sharedAIState.value?.providers ?? []).find(
        (item) => item.provider_id === displayProviderId.value,
    )
    return provider?.models.find((model) => model.enabled)?.id ?? ''
})
/** Placeholder text for a suggested-but-unchosen model, never a stored value. */
const displayModelLabel = computed(() => {
    if (props.modelId || !displayModelId.value) return undefined
    const provider = (sharedAIState.value?.providers ?? []).find(
        (item) => item.provider_id === displayProviderId.value,
    )
    return provider?.models.find((model) => model.id === displayModelId.value)?.name
})
const providerOptions = computed<ComboboxOption[]>(() => {
    const options: ComboboxOption[] = configuredProviders.value.map((provider) => ({
        value: provider.provider_id,
        label: providerLabel(provider.provider_id),
    }))
    // Show the stored provider as unavailable instead of swapping it for the
    // first configured one while it is filtered out.
    if (
        props.providerId &&
        !configuredProviders.value.some((item) => item.provider_id === props.providerId)
    ) {
        options.unshift({
            value: props.providerId,
            label: providerLabel(props.providerId),
            subLabel: formatMessage(messages.unavailableOption),
            disabled: true,
        })
    }
    return options
})

const modelOptions = computed<ComboboxOption[]>(() => {
    const providerId = displayProviderId.value
    const provider = (sharedAIState.value?.providers ?? []).find(
        (item) => item.provider_id === providerId,
    )
    const providerSelectable = configuredProviders.value.some(
        (item) => item.provider_id === providerId,
    )
    const options: ComboboxOption[] = (provider?.models ?? [])
        .filter((model) => model.enabled)
        .map((model) => ({
            value: model.id,
            label: model.name || model.id,
            subLabel: providerSelectable ? undefined : formatMessage(messages.unavailableOption),
            disabled: !providerSelectable,
        }))
    // Keep a stored model visible even when it is disabled or no longer listed.
    if (props.modelId && !options.some((option) => option.value === props.modelId)) {
        options.unshift({
            value: props.modelId,
            label:
                (provider?.models ?? []).find((model) => model.id === props.modelId)?.name ||
                props.modelId,
            subLabel: formatMessage(messages.unavailableOption),
            disabled: true,
        })
    }
    return options
})

const batchSizeOptions: ComboboxOption[] = [
    { value: '20', label: '20' },
    { value: '40', label: '40' },
    { value: '80', label: '80' },
]

/** Purely informational: an unavailable saved pair is kept, never replaced. */
const selectionUnavailable = computed(() => {
    if (!aiAvailable.value) return false
    if (!props.providerId) return false
    const provider = configuredProviders.value.some((item) => item.provider_id === props.providerId)
    if (!provider) return true
    if (!props.modelId) return false
    return !(sharedAIState.value?.providers ?? [])
        .find((item) => item.provider_id === props.providerId)
        ?.models.some((model) => model.enabled && model.id === props.modelId)
})

function selectProvider(value: string) {
    // A pick here is an explicit user action, so it may replace the pair.
    emit('update:providerId', value)
    const provider = configuredProviders.value.find((item) => item.provider_id === value)
    emit('update:modelId', provider?.models.find((model) => model.enabled)?.id ?? '')
}

function selectModel(value: string) {
    // Picking a model is also explicit. Confirm the provider shown next to it
    // in the same interaction when nothing had been chosen yet.
    if (!props.providerId && displayProviderId.value) {
        emit('update:providerId', displayProviderId.value)
    }
    emit('update:modelId', value)
}

/**
 * Restores the saved pair whenever it is still valid. This runs once on mount
 * regardless of whether the props already hold a value: the panel's own
 * transient provider filter may have replaced that value earlier.
 */
async function loadDefaults() {
    try {
        const [settingsResult, stateResult, catalogResult] = await Promise.allSettled([
            withTimeout(getTranslationSettings(), AI_LOAD_TIMEOUT_MS),
            withTimeout(getAIState(), AI_LOAD_TIMEOUT_MS),
            withTimeout(getAICatalog(), AI_LOAD_TIMEOUT_MS),
        ])
        if (catalogResult.status === 'fulfilled') aiCatalog.value = catalogResult.value
        aiLoadFailed.value = stateResult.status === 'rejected' && !sharedAIState.value
        const settings = settingsResult.status === 'fulfilled' ? settingsResult.value : null
        if (!settings?.ai_provider_id || !settings.ai_model_id) return
        const provider = (sharedAIState.value?.providers ?? []).find(
            (item) => item.provider_id === settings.ai_provider_id,
        )
        // Only restore a pair that still exists; a deleted provider or model
        // must not be resurrected as a phantom selection.
        if (!provider) return
        if (!provider.models.some((model) => model.id === settings.ai_model_id)) return
        if (
            props.providerId === settings.ai_provider_id &&
            props.modelId === settings.ai_model_id
        ) {
            return
        }
        emit('update:providerId', settings.ai_provider_id)
        emit('update:modelId', settings.ai_model_id)
    } finally {
        loading.value = false
    }
}

loadDefaults()
</script>

<template>
    <div class="flex flex-col gap-4">
        <div v-if="loading" class="text-sm text-[var(--color-text-tertiary)]">…</div>
        <template v-else>
            <div v-if="aiLoadFailed" class="text-sm text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.aiLoadError) }}
            </div>
            <div v-else-if="!aiAvailable" class="text-sm text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.aiNotConfigured) }}
            </div>
            <div v-else class="flex flex-col gap-3">
                <p
                    v-if="selectionUnavailable"
                    class="m-0 text-sm text-[var(--color-text-tertiary)]"
                >
                    {{ formatMessage(messages.selectionUnavailable) }}
                </p>
                <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
                    <label
                        class="flex flex-col gap-2 font-semibold text-[var(--color-text-primary)]"
                    >
                        {{ formatMessage(messages.provider) }}
                        <Combobox
                            :model-value="displayProviderId"
                            :display-value="displayProviderLabel"
                            :options="providerOptions"
                            @update:model-value="selectProvider"
                        >
                            <template #selected="{ label }">
                                <span class="inline-flex min-w-0 items-center gap-2">
                                    <AIIcon
                                        kind="provider-avatar"
                                        :value="displayProviderId"
                                        :size="20"
                                    />
                                    <span class="truncate">{{ label }}</span>
                                </span>
                            </template>
                            <template #option="{ item, isSelected }">
                                <div class="flex min-w-0 items-center gap-2.5">
                                    <AIIcon
                                        kind="provider-avatar"
                                        :value="String(item.value)"
                                        :size="22"
                                    />
                                    <div class="flex min-w-0 flex-col gap-1.5">
                                        <span
                                            class="truncate font-semibold leading-tight"
                                            :class="
                                                isSelected
                                                    ? 'text-brand'
                                                    : 'text-[var(--color-text-default)]'
                                            "
                                        >
                                            {{ item.label }}
                                        </span>
                                        <span
                                            v-if="item.subLabel"
                                            class="truncate text-sm text-[var(--color-text-tertiary)]"
                                        >
                                            {{ item.subLabel }}
                                        </span>
                                    </div>
                                </div>
                            </template>
                        </Combobox>
                    </label>
                    <label
                        class="flex flex-col gap-2 font-semibold text-[var(--color-text-primary)]"
                    >
                        {{ formatMessage(messages.model) }}
                        <div
                            class="model-combobox relative"
                            :class="{ 'has-model-icon': displayModelId }"
                        >
                            <AIIcon
                                v-if="displayModelId"
                                class="pointer-events-none absolute left-3 top-1/2 z-[2] -translate-y-1/2"
                                kind="model"
                                :value="displayModelId"
                                :size="20"
                            />
                            <Combobox
                                :model-value="displayModelId"
                                :display-value="displayModelLabel"
                                :options="modelOptions"
                                searchable
                                @update:model-value="selectModel(String($event))"
                            >
                                <template #option="{ item, isSelected }">
                                    <div class="flex min-w-0 items-center gap-2.5">
                                        <AIIcon
                                            kind="model"
                                            :value="String(item.value)"
                                            :size="22"
                                        />
                                        <div class="flex min-w-0 flex-col gap-1.5">
                                            <span
                                                class="truncate font-semibold leading-tight"
                                                :class="
                                                    isSelected
                                                        ? 'text-brand'
                                                        : 'text-[var(--color-text-default)]'
                                                "
                                            >
                                                {{ item.label }}
                                            </span>
                                            <span
                                                v-if="item.subLabel"
                                                class="truncate text-sm text-[var(--color-text-tertiary)]"
                                            >
                                                {{ item.subLabel }}
                                            </span>
                                        </div>
                                    </div>
                                </template>
                            </Combobox>
                        </div>
                    </label>
                </div>
            </div>
        </template>

        <div class="flex flex-col gap-3">
            <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                {{ formatMessage(messages.options) }}
            </h3>
            <label
                class="flex items-center justify-between gap-3 text-sm text-[var(--color-text-default)]"
            >
                <span>{{ formatMessage(messages.batchSize) }}</span>
                <div class="w-36">
                    <Combobox
                        :model-value="String(modelValue.batchSize)"
                        :options="batchSizeOptions"
                        @update:model-value="
                            emit('update:modelValue', { ...modelValue, batchSize: Number($event) })
                        "
                    />
                </div>
            </label>
            <label
                class="flex items-center justify-between gap-3 text-sm text-[var(--color-text-default)]"
            >
                <span>{{ formatMessage(messages.generateModName) }}</span>
                <Toggle
                    :model-value="modelValue.generateModName"
                    @update:model-value="
                        emit('update:modelValue', {
                            ...modelValue,
                            generateModName: Boolean($event),
                        })
                    "
                />
            </label>
            <label
                class="flex items-center justify-between gap-3 text-sm text-[var(--color-text-default)]"
            >
                <span>{{ formatMessage(messages.repairEnabled) }}</span>
                <Toggle
                    :model-value="modelValue.repairEnabled"
                    @update:model-value="
                        emit('update:modelValue', { ...modelValue, repairEnabled: Boolean($event) })
                    "
                />
            </label>
            <label
                class="flex items-center justify-between gap-3 text-sm text-[var(--color-text-default)]"
            >
                <span>{{ formatMessage(messages.classTextEnabled) }}</span>
                <Toggle
                    :model-value="modelValue.classTextEnabled"
                    @update:model-value="
                        emit('update:modelValue', {
                            ...modelValue,
                            classTextEnabled: Boolean($event),
                        })
                    "
                />
            </label>
        </div>
    </div>
</template>

<style scoped>
.model-combobox.has-model-icon :deep(input) {
    padding-left: 2.75rem !important;
}
</style>
