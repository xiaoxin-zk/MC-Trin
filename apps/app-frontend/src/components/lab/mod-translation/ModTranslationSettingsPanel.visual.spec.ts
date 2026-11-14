import '@/test/visual-styles.scss'

import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, type PropType, ref } from 'vue'

import ModTranslationSettingsPanel from './ModTranslationSettingsPanel.vue'

interface MockModel {
    id: string
    name: string
    enabled: boolean
    source: 'builtin' | 'custom' | 'remote'
}

interface MockProvider {
    provider_id: string
    custom_name: string | null
    protocol: 'openai'
    enabled: boolean
    endpoint: string
    settings: Record<string, string>
    has_api_key: boolean
    configured_credentials: string[]
    oauth_connected: boolean
    models: MockModel[]
}

interface StoredSettings {
    provider: string
    ai_provider_id: string
    ai_model_id: string
}

interface StubOption {
    value: string
    label: string
    subLabel?: string
    disabled?: boolean
}

const fixture = vi.hoisted(() => ({
    providers: [] as MockProvider[],
    stored: { provider: 'ai', ai_provider_id: '', ai_model_id: '' } as StoredSettings,
}))

vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    const Combobox = defineComponent({
        name: 'Combobox',
        props: {
            modelValue: { type: String, default: '' },
            options: { type: Array as PropType<StubOption[]>, default: () => [] },
        },
        emits: ['update:modelValue'],
        setup(props, { emit }) {
            return () =>
                h(
                    'div',
                    { class: 'combobox-stub', 'data-value': props.modelValue },
                    props.options.map((option) =>
                        h(
                            'button',
                            {
                                type: 'button',
                                class: 'combobox-option',
                                'data-option': option.value,
                                disabled: option.disabled,
                                onClick: () => {
                                    if (!option.disabled) emit('update:modelValue', option.value)
                                },
                            },
                            option.subLabel ? `${option.label} (${option.subLabel})` : option.label,
                        ),
                    ),
                )
        },
    })
    const Toggle = defineComponent({
        name: 'Toggle',
        props: { modelValue: Boolean },
        emits: ['update:modelValue'],
        setup:
            (props, { emit }) =>
            () =>
                h('button', {
                    type: 'button',
                    onClick: () => emit('update:modelValue', !props.modelValue),
                }),
    })
    return {
        Combobox,
        Toggle,
        defineMessages: (messages: unknown) => messages,
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage?: string }) => message?.defaultMessage ?? '',
        }),
    }
})
vi.mock('@/components/ui/settings/AIIcon.vue', async () => {
    const { defineComponent, h } = await import('vue')
    return { default: defineComponent({ name: 'AIIcon', setup: () => () => h('span') }) }
})
vi.mock('@/helpers/ai', async () => {
    const { ref } = await import('vue')
    const sharedAIState = ref<unknown>(null)
    return {
        sharedAIState,
        getAIState: async () => {
            const state = {
                settings: { enabled: true },
                catalog_source: '',
                providers: fixture.providers,
            }
            sharedAIState.value = state
            return state
        },
        getAICatalog: async () => [],
    }
})
vi.mock('@/helpers/translation', () => ({
    getTranslationSettings: async () => ({ ...fixture.stored }),
}))

function makeProvider(providerId: string, modelId: string, enabled = true): MockProvider {
    return {
        provider_id: providerId,
        custom_name: providerId,
        protocol: 'openai',
        enabled,
        endpoint: 'https://example.test/v1',
        settings: {},
        has_api_key: true,
        configured_credentials: [],
        oauth_connected: false,
        models: [{ id: modelId, name: modelId, enabled: true, source: 'custom' }],
    }
}

const defaultOptions = {
    batchSize: 40,
    deepBatchSize: 24,
    generateModName: false,
    repairEnabled: true,
    classTextEnabled: false,
    maxClassBatch: 16,
}

const wrappers: ReturnType<typeof mount>[] = []
afterEach(() => {
    wrappers.splice(0).forEach((wrapper) => wrapper.unmount())
    vi.clearAllMocks()
})

beforeEach(() => {
    fixture.providers = []
    fixture.stored = { provider: 'ai', ai_provider_id: '', ai_model_id: '' }
})

/**
 * Mounts the panel the way the lab page does: the provider/model live in the
 * page store, so the panel only changes them through `update:` events.
 */
async function renderWithStore(providerId: string, modelId: string) {
    const selection = { providerId: ref(providerId), modelId: ref(modelId) }
    const Host = defineComponent({
        setup() {
            const options = ref({ ...defaultOptions })
            return () =>
                h(ModTranslationSettingsPanel, {
                    modelValue: options.value,
                    'onUpdate:modelValue': (value: typeof options.value) => {
                        options.value = value
                    },
                    providerId: selection.providerId.value,
                    'onUpdate:providerId': (value: string) => {
                        selection.providerId.value = value
                    },
                    modelId: selection.modelId.value,
                    'onUpdate:modelId': (value: string) => {
                        selection.modelId.value = value
                    },
                })
        },
    })
    const wrapper = mount(Host, { attachTo: document.body })
    wrappers.push(wrapper)
    await flushPromises()
    return { wrapper, selection }
}

it('restores the stored pair on a warm second visit instead of keeping providers[0]', async () => {
    // Second visit: the page store still carries the pair that the previous
    // visit replaced with the first configured provider.
    fixture.providers = [
        makeProvider('codeflow', 'model-b', false),
        makeProvider('other', 'other-model'),
    ]
    fixture.stored = { provider: 'ai', ai_provider_id: 'codeflow', ai_model_id: 'model-b' }

    const { selection } = await renderWithStore('other', 'other-model')

    // The stored pair is restored because both ids still exist, even though the
    // provider is currently filtered out as disabled.
    expect(selection.providerId.value).toBe('codeflow')
    expect(selection.modelId.value).toBe('model-b')
})

it('never emits a replacement while the saved provider is temporarily filtered out', async () => {
    fixture.providers = [
        makeProvider('codeflow', 'model-b', false),
        makeProvider('other', 'other-model'),
    ]
    fixture.stored = { provider: 'ai', ai_provider_id: 'codeflow', ai_model_id: 'model-b' }

    // The store still holds the saved pair; only the provider list changed.
    const { wrapper, selection } = await renderWithStore('codeflow', 'model-b')
    await flushPromises()

    expect(selection.providerId.value).toBe('codeflow')
    expect(selection.modelId.value).toBe('model-b')

    // The saved provider stays visible as an unavailable option instead of
    // being swapped for the first configured provider.
    const providerCombobox = wrapper
        .findAll('.combobox-stub')
        .find((combobox) => combobox.find('[data-option="codeflow"]').exists())!
    expect(providerCombobox.attributes('data-value')).toBe('codeflow')
    const storedOption = providerCombobox.find('[data-option="codeflow"]')
    expect(storedOption.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('The saved AI provider or model is unavailable')
})

it('saves the new pair after an explicit provider pick', async () => {
    fixture.providers = [makeProvider('codeflow', 'model-b'), makeProvider('other', 'other-model')]
    fixture.stored = { provider: 'ai', ai_provider_id: 'codeflow', ai_model_id: 'model-b' }

    const { wrapper, selection } = await renderWithStore('codeflow', 'model-b')
    expect(selection.providerId.value).toBe('codeflow')

    const providerCombobox = wrapper
        .findAll('.combobox-stub')
        .find((combobox) => combobox.find('[data-option="other"]').exists())!
    await providerCombobox.find('[data-option="other"]').trigger('click')
    await flushPromises()

    expect(selection.providerId.value).toBe('other')
    expect(selection.modelId.value).toBe('other-model')
})
