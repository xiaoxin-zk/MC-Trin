import { flushPromises, mount } from '@vue/test-utils'
import { expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { GameInstance } from '@/helpers/types'

import SyncSettings from './SyncSettings.vue'

const backend = vi.hoisted(() => ({
    setOption: vi.fn(),
    updated: vi.fn(),
    capabilities: [
        { option: 'data_packs', supported: true, disabled_reason: null },
        { option: 'screenshots', supported: true, disabled_reason: null },
    ],
}))

vi.mock('@/providers/instance-settings', () => ({
    injectInstanceSettings: () => ({
        instance: ref({
            id: 'instance-a',
            synced_options: { data_packs: false, screenshots: false },
        } as GameInstance),
        onInstanceUpdated: backend.updated,
    }),
}))

vi.mock('@/helpers/instance', () => ({
    get_synced_options_overview: async () => ({
        global_options: { data_packs: true },
        capabilities: backend.capabilities,
    }),
    set_instance_synced_option: backend.setOption,
}))

vi.mock('@/pages/instance/query-options', () => ({ instanceKeys: { all: ['instances'] } }))

vi.mock('@tanstack/vue-query', async () => {
    const { ref } = await import('vue')
    return {
        useQuery: () => ({
            data: ref({
                capabilities: backend.capabilities,
            }),
        }),
        useMutation: (options: {
            mutationFn: (variables: { option: string; enabled: boolean }) => Promise<GameInstance>
            onSuccess: (instance: GameInstance) => Promise<void>
        }) => ({
            isPending: ref(false),
            mutate: async (variables: { option: string; enabled: boolean }) => {
                await options.onSuccess(await options.mutationFn(variables))
            },
        }),
        useQueryClient: () => ({ invalidateQueries: async () => {}, setQueryData: () => {} }),
    }
})

vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    return {
        defineMessages: (messages: unknown) => messages,
        injectNotificationManager: () => ({ handleError: vi.fn() }),
        Toggle: defineComponent({
            props: ['modelValue', 'disabled'],
            emits: ['update:modelValue'],
            setup(props, { attrs, emit }) {
                return () =>
                    h(
                        'button',
                        {
                            ...attrs,
                            disabled: props.disabled,
                            onClick: () => emit('update:modelValue', !props.modelValue),
                        },
                        attrs['aria-label'],
                    )
            },
        }),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})

it('exposes the data pack sync toggle and saves its per-instance preference', async () => {
    backend.capabilities = [
        { option: 'data_packs', supported: true, disabled_reason: null },
        { option: 'screenshots', supported: true, disabled_reason: null },
    ]
    backend.setOption.mockReset().mockResolvedValue({
        id: 'instance-a',
        synced_options: { data_packs: true },
    })
    backend.updated.mockReset()
    const wrapper = mount(SyncSettings)
    await flushPromises()

    await wrapper.get('button[aria-label="Data packs"]').trigger('click')
    await flushPromises()

    expect(backend.setOption).toHaveBeenCalledWith('instance-a', 'data_packs', true)
    expect(backend.updated).toHaveBeenCalledWith({
        id: 'instance-a',
        synced_options: { data_packs: true },
    })
})

it('exposes the screenshot gallery preference and saves it per instance', async () => {
    backend.capabilities = [
        { option: 'data_packs', supported: true, disabled_reason: null },
        { option: 'screenshots', supported: true, disabled_reason: null },
    ]
    backend.setOption.mockReset().mockResolvedValue({
        id: 'instance-a',
        synced_options: { screenshots: true },
    })
    const wrapper = mount(SyncSettings)
    await flushPromises()

    await wrapper.get('button[aria-label="Include in global screenshot gallery"]').trigger('click')
    await flushPromises()

    expect(backend.setOption).toHaveBeenCalledWith('instance-a', 'screenshots', true)
})

it('disables screenshot gallery preference when the backend marks it unsupported', async () => {
    backend.capabilities = [
        { option: 'data_packs', supported: true, disabled_reason: null },
        { option: 'screenshots', supported: false, disabled_reason: 'not_available' },
    ]
    const wrapper = mount(SyncSettings)
    await flushPromises()

    expect(
        (
            wrapper.get('button[aria-label="Include in global screenshot gallery"]')
                .element as HTMLButtonElement
        ).disabled,
    ).toBe(true)
})
