import { mount } from '@vue/test-utils'
import { expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import ConfirmRemoveWorldModal from './ConfirmRemoveWorldModal.vue'

vi.mock('@modrinth/assets', () => ({
    TrashIcon: defineComponent(() => () => h('span')),
    XIcon: defineComponent(() => () => h('span')),
}))

vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    const newModal = defineComponent({
        setup(_props, { slots, expose }) {
            expose({ show: () => {}, hide: () => {} })
            return () => h('div', [slots.default?.(), slots.actions?.()])
        },
    })
    const radioButtons = defineComponent({
        props: ['items', 'modelValue'],
        emits: ['update:modelValue'],
        setup(props, { emit, slots }) {
            return () =>
                h(
                    'div',
                    props.items.map((item: string) =>
                        h(
                            'button',
                            { onClick: () => emit('update:modelValue', item) },
                            slots.default?.({ item }),
                        ),
                    ),
                )
        },
    })
    const button = defineComponent({
        props: ['disabled'],
        emits: ['click'],
        setup(_props, { emit, slots }) {
            return () => h('button', { onClick: () => emit('click') }, slots.default?.())
        },
    })
    const admonition = defineComponent({
        setup(_props, { slots }) {
            return () => h('section', slots.default?.())
        },
    })
    const cancel = { defaultMessage: 'Cancel' }
    return {
        Admonition: admonition,
        Button: button,
        NewModal: newModal,
        RadioButtons: radioButtons,
        commonMessages: { cancelButton: cancel },
        defineMessages: (messages: unknown) => messages,
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})

const syncedServer = {
    type: 'server' as const,
    name: 'Lobby',
    display_status: 'normal' as const,
    synced_server_id: 'server-id',
    index: 0,
    address: 'play.example.test',
    pack_status: 'prompt' as const,
}

it('defaults to removing a synced server only from this instance', async () => {
    const wrapper = mount(ConfirmRemoveWorldModal, {
        props: { world: syncedServer },
    })

    await wrapper.findAll('button').at(-1)!.trigger('click')

    expect(wrapper.emitted('confirm')?.[0]).toEqual([syncedServer, 'keep_in_other_instances'])
})

it('passes the all-instances removal mode when selected', async () => {
    const wrapper = mount(ConfirmRemoveWorldModal, {
        props: { world: syncedServer },
    })

    await wrapper.findAll('button').at(1)!.trigger('click')
    await wrapper.findAll('button').at(-1)!.trigger('click')

    expect(wrapper.emitted('confirm')?.[0]).toEqual([syncedServer, 'remove_from_other_instances'])
})
