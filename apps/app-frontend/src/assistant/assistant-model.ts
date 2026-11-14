import type { AIProviderConfig, AIProviderDefinition } from '@/helpers/ai'

/** One selectable text model, qualified by the provider that serves it. */
export interface AssistantModelOption {
    providerId: string
    modelId: string
    /** `display_name` when the provider stores one, otherwise the model id. */
    modelLabel: string
    providerLabel: string
}

export interface AssistantModelSelection {
    providerId: string
    modelId: string
}

/**
 * Provider and model are selected together, so one `<option>` value has to
 * carry both halves. `\u001f` (unit separator) cannot appear in a provider id
 * and is trimmed out of model ids by the backend, so it is unambiguous.
 */
const SEPARATOR = '\u001f'

export function assistantModelKey(providerId: string, modelId: string): string {
    return `${providerId}${SEPARATOR}${modelId}`
}

export function parseAssistantModelValue(value: string): AssistantModelSelection | null {
    const index = value.indexOf(SEPARATOR)
    if (index <= 0 || index === value.length - 1) return null
    return {
        providerId: value.slice(0, index),
        modelId: value.slice(index + 1),
    }
}

/**
 * A provider can only answer when it is enabled and holds a credential its
 * protocol accepts; a keyless local provider (Ollama) counts as configured.
 */
export function providerIsUsable(
    provider: AIProviderConfig,
    definition: AIProviderDefinition | undefined,
): boolean {
    if (!provider.enabled) return false
    return (
        definition?.auth_type === 'none' ||
        provider.has_api_key ||
        provider.oauth_connected ||
        provider.configured_credentials.length > 0
    )
}

export function providerLabel(
    provider: AIProviderConfig,
    definition: AIProviderDefinition | undefined,
): string {
    return provider.custom_name?.trim() || definition?.name || provider.provider_id
}

/** Every enabled model of every usable provider, in catalog order. */
export function assistantModelOptions(
    providers: readonly AIProviderConfig[],
    catalog: readonly AIProviderDefinition[],
): AssistantModelOption[] {
    const options: AssistantModelOption[] = []
    for (const provider of providers) {
        const definition = catalog.find((item) => item.id === provider.provider_id)
        if (!providerIsUsable(provider, definition)) continue
        for (const model of provider.models) {
            if (!model.enabled) continue
            options.push({
                providerId: provider.provider_id,
                modelId: model.id,
                modelLabel: model.name?.trim() || model.id,
                providerLabel: providerLabel(provider, definition),
            })
        }
    }
    return options
}

/** The stored choice, but only while that provider and model are still usable. */
export function findAssistantModel(
    options: readonly AssistantModelOption[],
    selection: AssistantModelSelection | null,
): AssistantModelOption | null {
    if (!selection) return null
    return (
        options.find(
            (option) =>
                option.providerId === selection.providerId && option.modelId === selection.modelId,
        ) ?? null
    )
}

export interface AssistantModelGroup {
    providerId: string
    label: string
    options: AssistantModelOption[]
}

/**
 * Groups the options by provider so a long list stays navigable: the picker
 * renders one `<optgroup>` per provider instead of a flat run of model names.
 */
export function groupAssistantModels(
    options: readonly AssistantModelOption[],
): AssistantModelGroup[] {
    const groups: AssistantModelGroup[] = []
    for (const option of options) {
        const existing = groups.at(-1)
        if (existing?.providerId === option.providerId) {
            existing.options.push(option)
            continue
        }
        groups.push({
            providerId: option.providerId,
            label: option.providerLabel,
            options: [option],
        })
    }
    return groups
}
