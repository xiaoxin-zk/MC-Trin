import { flushPromises, mount } from '@vue/test-utils'
import { expect, it, vi } from 'vitest'
import { defineComponent, h, Suspense } from 'vue'

import Settings from './Settings.vue'

const fixture = vi.hoisted(() => ({
    finish: {} as Record<string, () => void>,
    replace: vi.fn().mockResolvedValue(undefined),
    route: { hash: '#interface', query: {} } as {
        hash: string
        query: Record<string, unknown>
    },
}))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '1.0' }))
vi.mock('@tauri-apps/plugin-os', () => ({ platform: () => 'windows', version: () => '11' }))
vi.mock('@/store/state', () => ({ useTheming: () => ({ devMode: false }) }))
vi.mock('@/helpers/settings', () => ({ update: vi.fn() }))
vi.mock('@/providers/download-progress', async () => {
    const { ref } = await import('vue')
    return { injectAppUpdateDownloadProgress: () => ({ progress: ref(null), version: ref(null) }) }
})
vi.mock('vue-router', () => ({
    useRoute: () => fixture.route,
    useRouter: () => ({ replace: fixture.replace }),
}))
vi.mock('@modrinth/ui', () => ({
    defineMessages: (value: unknown) => value,
    ProgressBar: { render: () => null },
    useLoadingBarToken: () => {},
    injectNotificationManager: () => ({ handleError: vi.fn() }),
    useVIntl: () => ({
        formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
    }),
}))
vi.mock('@/components/ui/settings/settings-search-index', () => ({
    getSettingsSearchTargetId: (entry: { id: string }) => `settings-target-${entry.id}`,
}))
vi.mock('@/components/ui/settings/settings-registry', async () => {
    const { defineAsyncComponent, h, markRaw } = await import('vue')
    const message = (text: string) => ({ id: text, defaultMessage: text })
    const categories = [
        {
            id: 'interface',
            group: 'launcher',
            name: message('Appearance'),
            entries: [],
            content: markRaw({ render: () => h('p', 'Appearance content') }),
        },
        ...['slow', 'latest'].map((id) => ({
            id,
            group: 'launcher',
            name: message(id),
            icon: markRaw({ render: () => null }),
            entries: [{ id: `${id}-target`, label: message(`${id} setting`) }],
            content: defineAsyncComponent(
                () =>
                    new Promise((resolve) => {
                        fixture.finish[id] = () =>
                            resolve({
                                render: () => h('input', { id: `settings-target-${id}-target` }),
                            })
                    }),
            ),
        })),
    ]
    return {
        getVisibleSettingsCategories: () => categories,
        getVisibleSettingsGroups: () => [{ id: 'launcher', name: message('Launcher'), categories }],
        settingsPageTitle: message('Settings'),
    }
})

it('waits for async settings to resolve and only locates the latest selected result', async () => {
    fixture.route.hash = '#interface'
    fixture.route.query = {}
    const wrapper = mount(
        defineComponent({ render: () => h(Suspense, null, { default: () => h(Settings) }) }),
        { attachTo: document.body },
    )
    try {
        await flushPromises()
        const input = wrapper.get<HTMLInputElement>('input[type="search"]')
        await input.setValue('setting')
        await vi.waitFor(() => expect(wrapper.findAll('.settings-search-result')).toHaveLength(2))
        await wrapper.findAll('.settings-search-result')[0].trigger('click')
        await vi.waitFor(() => expect(fixture.finish.slow).toBeDefined())
        await new Promise((resolve) => setTimeout(resolve, 350))
        expect(input.element.value).toBe('setting')
        await wrapper.findAll('.settings-search-result')[1].trigger('click')
        await vi.waitFor(() => expect(fixture.finish.latest).toBeDefined())
        fixture.finish.slow()
        await flushPromises()
        expect(input.element.value).toBe('setting')
        fixture.finish.latest()
        await vi.waitFor(() =>
            expect(wrapper.get('#settings-target-latest-target').classes()).toContain(
                'settings-search-result-highlight',
            ),
        )
        expect(input.element.value).toBe('')
        expect(wrapper.find('#settings-target-slow-target').exists()).toBe(false)
    } finally {
        wrapper.unmount()
    }
})

it('opens and highlights the control a ?target= deep link names', async () => {
    // "Configure AI" in log diagnosis opens launch defaults with
    // `?target=log-share`. Without the route-driven reveal the user landed on
    // the right tab but nowhere near the provider and model pickers, which is
    // the defect this pins.
    fixture.route.hash = '#latest'
    fixture.route.query = { target: 'latest-target' }
    const wrapper = mount(
        defineComponent({ render: () => h(Suspense, null, { default: () => h(Settings) }) }),
        { attachTo: document.body },
    )
    try {
        await flushPromises()
        fixture.finish.latest()
        await vi.waitFor(() =>
            expect(wrapper.get('#settings-target-latest-target').classes()).toContain(
                'settings-search-result-highlight',
            ),
        )
    } finally {
        wrapper.unmount()
    }
})

it('leaves a deep link alone when its target id is not an anchor id', async () => {
    // A hand-written link must not be able to address an arbitrary element:
    // the resolver only accepts the plain ids the settings index uses.
    fixture.route.hash = '#latest'
    fixture.route.query = { target: '../etc' }
    const wrapper = mount(
        defineComponent({ render: () => h(Suspense, null, { default: () => h(Settings) }) }),
        { attachTo: document.body },
    )
    try {
        await flushPromises()
        fixture.finish.latest()
        await vi.waitFor(() =>
            expect(wrapper.find('#settings-target-latest-target').exists()).toBe(true),
        )
        expect(wrapper.get('#settings-target-latest-target').classes()).not.toContain(
            'settings-search-result-highlight',
        )
    } finally {
        wrapper.unmount()
    }
})
