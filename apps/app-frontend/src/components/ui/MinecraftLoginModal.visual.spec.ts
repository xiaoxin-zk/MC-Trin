import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import MinecraftLoginModal from './MinecraftLoginModal.vue'

const backend = vi.hoisted(() => ({ begin: vi.fn(), poll: vi.fn(), open: vi.fn() }))
vi.mock('@/helpers/auth', () => ({
    begin_device_login: backend.begin,
    poll_device_login: backend.poll,
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: backend.open }))
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h, ref } = await import('vue')
    return {
        Button: defineComponent({
            props: ['disabled'],
            setup:
                (props, { slots }) =>
                () =>
                    h('button', { disabled: props.disabled }, slots.default?.()),
        }),
        NewModal: defineComponent({
            props: ['header', 'onHide'],
            setup(props, { slots, expose }) {
                const visible = ref(false)
                expose({
                    show: () => {
                        visible.value = true
                    },
                    hide: () => {
                        visible.value = false
                        props.onHide?.()
                    },
                })
                return () =>
                    visible.value
                        ? h('section', { 'aria-label': props.header }, slots.default?.())
                        : null
            },
        }),
        defineMessages: (messages: unknown) => messages,
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})

const flow = {
    device_code: 'test-device',
    user_code: 'ABCD-EFGH',
    verification_uri: 'https://microsoft.com/devicelogin',
    expires_in: 900,
    interval: 5,
}
const cleanups: (() => void)[] = []
beforeEach(() => {
    vi.useFakeTimers()
    vi.resetAllMocks()
    backend.begin.mockResolvedValue(flow)
    backend.poll.mockResolvedValue({ status: 'pending', slow_down: false })
    backend.open.mockResolvedValue(undefined)
})
afterEach(() => {
    cleanups.splice(0).forEach((cleanup) => cleanup())
    vi.useRealTimers()
})

function show() {
    const wrapper = mount(MinecraftLoginModal)
    cleanups.push(() => wrapper.unmount())
    wrapper.vm.showDeviceLogin()
    return wrapper
}

it('shows the Microsoft code and opens its verification page', async () => {
    const wrapper = show()
    await flushPromises()
    expect(wrapper.find('code').text()).toBe(flow.user_code)
    expect(backend.poll).not.toHaveBeenCalled()
    await wrapper.find('button').trigger('click')
    expect(backend.open).toHaveBeenCalledWith(flow.verification_uri)
})

it('emits the saved Microsoft account once and closes after a successful poll', async () => {
    const credential = {
        account_id: 'microsoft:test-player',
        account_type: 'microsoft',
        profile: { id: 'test-player', name: 'TestPlayer' },
    }
    backend.poll.mockResolvedValue({ status: 'complete', credentials: credential })
    const wrapper = show()
    await flushPromises()
    await vi.advanceTimersByTimeAsync(5000)
    await flushPromises()
    expect(wrapper.emitted('complete')).toEqual([[credential]])
    expect(wrapper.find('section').exists()).toBe(false)
    await vi.advanceTimersByTimeAsync(30000)
    expect(backend.poll).toHaveBeenCalledOnce()
})

it('lets the user request another code after an initial request fails', async () => {
    backend.begin.mockRejectedValueOnce(new Error('Microsoft is unavailable'))
    const wrapper = show()
    await flushPromises()
    expect(wrapper.text()).toContain('Microsoft is unavailable')
    await wrapper.find('button').trigger('click')
    await flushPromises()
    expect(backend.begin).toHaveBeenCalledTimes(2)
    expect(wrapper.find('code').text()).toBe(flow.user_code)
    expect(wrapper.text()).not.toContain('Microsoft is unavailable')
})

it('shows browser-opening failures and keeps the code available', async () => {
    backend.open.mockRejectedValue(new Error('No browser is configured'))
    const wrapper = show()
    await flushPromises()
    await wrapper.find('button').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('No browser is configured')
    expect(wrapper.find('code').text()).toBe(flow.user_code)
})

it('cancels polling when the user closes the modal', async () => {
    const wrapper = show()
    await flushPromises()
    wrapper.vm.hide()
    await vi.advanceTimersByTimeAsync(30000)
    expect(backend.poll).not.toHaveBeenCalled()
    expect(wrapper.emitted('complete')).toBeUndefined()
})
