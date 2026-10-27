import { flushPromises, mount } from '@vue/test-utils'
import { expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import GameSettingsModal from './index.vue'

const fixture = vi.hoisted(() => ({ retryPreview: vi.fn() }))

vi.mock('@modrinth/assets', async () => {
    const { defineComponent, h } = await import('vue')
    const icon = defineComponent({ setup: () => () => h('span', { 'aria-hidden': 'true' }) })
    return Object.fromEntries(
        [
            'EyeIcon',
            'LanguagesIcon',
            'LinkIcon',
            'MessageIcon',
            'MonitorIcon',
            'RefreshCwIcon',
            'SaveIcon',
            'SearchIcon',
            'SettingsIcon',
            'ShirtIcon',
            'TagCategoryAudioIcon',
            'TagCategoryGamepad2Icon',
            'UnlinkIcon',
            'WrenchIcon',
            'XIcon',
        ].map((name) => [name, icon]),
    )
})
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    const button = defineComponent({
        setup:
            (_, { attrs, slots }) =>
            () =>
                h('button', attrs, slots.default?.()),
    })
    return {
        Button: button,
        commonMessages: {
            cancelButton: { defaultMessage: 'Cancel' },
            retryButton: { defaultMessage: 'Retry' },
        },
        defineMessages: (messages: unknown) => messages,
        FloatingActionBar: defineComponent({
            setup:
                (_, { slots }) =>
                () =>
                    h('div', slots.default?.()),
        }),
        StyledInput: defineComponent({
            props: ['modelValue'],
            emits: ['update:modelValue'],
            setup: () => () => h('input'),
        }),
        TabbedModal: defineComponent({
            setup: (_, { expose, slots }) => {
                expose({ show: vi.fn(), hide: vi.fn(), setTab: vi.fn() })
                return () =>
                    h('div', [
                        slots['sidebar-header']?.(),
                        slots.content?.(),
                        slots['floating-action-bar']?.(),
                    ])
            },
        }),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))
vi.mock('@/pages/instance/components/settings-modal/instance-settings-context', () => ({
    injectInstanceSettings: () => null,
}))
vi.mock('@/providers/app-settings-modal', () => ({ appSettingsModalContextKey: Symbol() }))
vi.mock('./use-editor', async () => {
    const { ref } = await import('vue')
    return {
        useGameSettingsEditor: () => ({
            draftState: ref({ categories: [], settings: [] }),
            isLocalEditor: ref(false),
            isDirty: ref(false),
            hasBlockingDraft: ref(false),
            loading: ref(false),
            loadError: ref(false),
            previewError: ref('Could not check these changes. Retry before saving.'),
            saveResult: ref(null),
            saving: ref(false),
            load: vi.fn(),
            reset: vi.fn(),
            cancelChanges: vi.fn(),
            setSyncEnabled: vi.fn(),
            setCanonicalValue: vi.fn(),
            save: vi.fn(),
            saveAndWait: vi.fn(async () => true),
            retryPreview: fixture.retryPreview,
        }),
    }
})
vi.mock('./use-labels', async () => {
    const { ref } = await import('vue')
    return { useGameSettingLabels: () => ref({}) }
})
vi.mock('./row.vue', () => ({ default: defineComponent({ setup: () => () => h('div') }) }))

it('shows the preview error and retries the validation request', async () => {
    fixture.retryPreview.mockReset()
    const wrapper = mount(GameSettingsModal)

    expect(wrapper.get('[role="alert"]').text()).toContain('Could not check these changes')
    await wrapper
        .findAll('button')
        .find((button) => button.text().trim() === 'Retry')!
        .trigger('click')
    await flushPromises()

    expect(fixture.retryPreview).toHaveBeenCalledOnce()
})
