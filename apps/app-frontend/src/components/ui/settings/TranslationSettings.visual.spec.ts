import '@/test/visual-styles.scss'

import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { PropType } from 'vue'

import TranslationSettings from './TranslationSettings.vue'

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
    provider: 'bilibili' | 'google' | 'deepl' | 'ai'
    target_language: string
    mode: 'bilingual' | 'translation-only'
    auto_translate: boolean
    style: string
    ai_provider_id: string
    ai_model_id: string
    ai_system_prompt: string
    deepl_api_endpoint: string
    deepl_api_key: string | null
}

interface StubOption {
    value: string
    label: string
    subLabel?: string
    disabled?: boolean
}

const fixture = vi.hoisted(() => ({
    providers: [] as MockProvider[],
    stored: {} as StoredSettings,
    saves: [] as StoredSettings[],
}))

vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    // Renders the current value and every option, including disabled ones, so
    // the test can assert what the user actually sees.
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
    const passthrough = (name: string) =>
        defineComponent({
            name,
            setup:
                (_, { slots }) =>
                () =>
                    h('div', slots.default?.()),
        })
    return {
        Button: passthrough('Button'),
        Card: passthrough('Card'),
        Combobox,
        defineMessages: (messages: unknown) => messages,
        injectNotificationManager: () => ({ handleError: vi.fn() }),
        LOCALES: [
            {
                code: 'en-US',
                name: 'English',
                translatedName: { id: 'locale.en-US', defaultMessage: 'English' },
            },
            {
                code: 'zh-CN',
                name: '中文',
                translatedName: { id: 'locale.zh-CN', defaultMessage: 'Chinese' },
            },
        ],
        StyledInput: defineComponent({
            props: { modelValue: { type: String, default: '' } },
            emits: ['update:modelValue'],
            setup: () => () => h('input'),
        }),
        Toggle: defineComponent({
            name: 'Toggle',
            props: { modelValue: Boolean },
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('button', {
                        class: 'toggle-stub',
                        type: 'button',
                        onClick: () => emit('update:modelValue', !props.modelValue),
                    }),
        }),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage?: string }) => message?.defaultMessage ?? '',
        }),
    }
})
vi.mock('@modrinth/assets', async () => {
    const { defineComponent, h } = await import('vue')
    const icon = defineComponent({ name: 'Icon', setup: () => () => h('i') })
    return { PlugIcon: icon, SpinnerIcon: icon, TrashIcon: icon }
})
vi.mock('./AIIcon.vue', async () => {
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
    clearTranslationCache: async () => {},
    getGoogleIpPoolSize: async () => 0,
    getTranslationErrorKind: () => 'provider',
    getTranslationSettings: async () => ({ ...fixture.stored }),
    testTranslationProvider: async () => 'ok',
    updateTranslationSettings: async (settings: StoredSettings) => {
        fixture.saves.push({ ...settings })
    },
}))

const wrappers: ReturnType<typeof mount>[] = []
function mountTracked() {
    const wrapper = mount(TranslationSettings, { attachTo: document.body })
    wrappers.push(wrapper)
    return wrapper
}
function unmountTracked(wrapper: ReturnType<typeof mount>) {
    const index = wrappers.indexOf(wrapper)
    if (index >= 0) wrappers.splice(index, 1)
    wrapper.unmount()
}
afterEach(() => {
    wrappers.splice(0).forEach((wrapper) => wrapper.unmount())
    vi.clearAllMocks()
})

beforeEach(() => {
    if (!document.querySelector('#teleports')) {
        const teleports = document.createElement('div')
        teleports.id = 'teleports'
        document.body.append(teleports)
    }
    fixture.providers = []
    fixture.saves = []
    fixture.stored = {
        provider: 'ai',
        target_language: '',
        mode: 'bilingual',
        auto_translate: true,
        style: 'weakened',
        ai_provider_id: '',
        ai_model_id: '',
        ai_system_prompt: '',
        deepl_api_endpoint: 'https://api-free.deepl.com/v2/translate',
        deepl_api_key: null,
    }
})

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

function comboboxOffering(wrapper: ReturnType<typeof mount>, optionValue: string) {
    const found = wrapper
        .findAll('.combobox-stub')
        .find((combobox) => combobox.find(`[data-option="${optionValue}"]`).exists())
    if (!found) throw new Error(`No combobox offers ${optionValue}`)
    return found
}

it('keeps and never persists over the stored AI pair while the provider is unavailable', async () => {
    // The saved provider is disabled for now, so the enabled-provider filter
    // drops it even though the saved pair still exists.
    fixture.providers = [
        makeProvider('codeflow', 'model-b', false),
        makeProvider('other', 'other-model'),
    ]
    fixture.stored.ai_provider_id = 'codeflow'
    fixture.stored.ai_model_id = 'model-b'

    const wrapper = mountTracked()
    await flushPromises()

    // The stored pair is displayed as unavailable rather than replaced by the
    // first selectable provider.
    const providerCombobox = comboboxOffering(wrapper, 'codeflow')
    expect(providerCombobox.attributes('data-value')).toBe('codeflow')
    const storedOption = providerCombobox.find('[data-option="codeflow"]')
    expect(storedOption.attributes('disabled')).toBeDefined()
    expect(storedOption.text()).toContain('Unavailable')
    expect(wrapper.text()).toContain('The saved AI provider is unavailable')

    // An unrelated edit schedules the autosave; it must carry the pair that is
    // still on disk rather than the disabled one's replacement.
    await wrapper.get('button.toggle-stub').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 350))
    expect(fixture.saves.length).toBeGreaterThan(0)
    expect(fixture.saves.at(-1)).toMatchObject({
        ai_provider_id: 'codeflow',
        ai_model_id: 'model-b',
    })

    // The unmount flush must not repair the unavailable selection either.
    await wrapper.get('button.toggle-stub').trigger('click')
    unmountTracked(wrapper)
    await flushPromises()
    expect(fixture.saves.at(-1)).toMatchObject({
        ai_provider_id: 'codeflow',
        ai_model_id: 'model-b',
    })
    expect(fixture.saves.length).toBeGreaterThan(1)
})

it('saves the new AI pair only after an explicit pick', async () => {
    fixture.providers = [makeProvider('codeflow', 'model-b'), makeProvider('other', 'other-model')]
    fixture.stored.ai_provider_id = 'codeflow'
    fixture.stored.ai_model_id = 'model-b'

    const wrapper = mountTracked()
    await flushPromises()
    expect(fixture.saves).toEqual([])

    await comboboxOffering(wrapper, 'other').find('[data-option="other"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 350))

    expect(fixture.saves.at(-1)).toMatchObject({
        ai_provider_id: 'other',
        ai_model_id: 'other-model',
    })
})
