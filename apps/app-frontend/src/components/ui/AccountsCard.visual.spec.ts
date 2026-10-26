import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, Suspense } from 'vue'

import { useNetworkStatus } from '@/composables/useNetworkStatus'

import AccountsCard from './AccountsCard.vue'

const backend = vi.hoisted(() => ({
    users: vi.fn(),
    defaultUser: vi.fn(),
    setDefault: vi.fn(),
    heads: vi.fn(),
    skins: vi.fn(),
    error: vi.fn(),
    offline: vi.fn(),
    beginYggdrasil: vi.fn(),
    finishYggdrasil: vi.fn(),
    savedLogins: vi.fn(),
    rememberedPassword: vi.fn(),
    savePassword: vi.fn(),
    deviceLogin: vi.fn(),
}))

vi.mock('@/helpers/auth', () => ({
    users: backend.users,
    get_default_user: backend.defaultUser,
    add_offline_user: backend.offline,
    begin_yggdrasil_login: backend.beginYggdrasil,
    delete_yggdrasil_password: vi.fn(),
    finish_yggdrasil_login: backend.finishYggdrasil,
    get_yggdrasil_password: backend.rememberedPassword,
    list_yggdrasil_saved_logins: backend.savedLogins,
    login: vi.fn(),
    remove_user: vi.fn(),
    set_default_user: backend.setDefault,
    set_yggdrasil_password: backend.savePassword,
}))
vi.mock('@/helpers/skins', () => ({ get_available_skins: backend.skins }))
vi.mock('@/helpers/rendering/batch-skin-renderer.ts', () => ({
    getPlayerHeadUrl: backend.heads,
    retainPlayerHeadTextures: () => () => {},
}))
vi.mock('@/helpers/events', () => ({ process_listener: async () => () => {} }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))
vi.mock('@tanstack/vue-query', () => ({ useQueryClient: () => ({}) }))
vi.mock('@/helpers/analytics', () => ({ trackEvent: vi.fn() }))
vi.mock('@/store/error.js', () => ({
    handleSevereError: vi.fn(),
    useError: () => ({ showAntiPiracyNotice: vi.fn() }),
}))
vi.mock('@/composables/useAntiPiracyStatus', async () => {
    const { ref } = await import('vue')
    return {
        isOfflineAccountRestrictedError: () => false,
        useAntiPiracyStatus: () => ({
            status: ref({ restricted: false }),
            refresh: vi.fn(),
        }),
    }
})
vi.mock('vue-router', () => ({ useRoute: () => ({ fullPath: '/' }) }))
vi.mock('@/components/ui/MinecraftLoginModal.vue', async () => {
    const { defineComponent } = await import('vue')
    return {
        default: defineComponent({
            setup(_, { expose }) {
                expose({ showDeviceLogin: backend.deviceLogin })
                return () => null
            },
        }),
    }
})
vi.mock('@modrinth/ui', async () => {
    const { h, defineComponent, ref } = await import('vue')
    const container = defineComponent({
        setup(_, { slots }) {
            return () => h('div', [slots.title?.(), slots.default?.()])
        },
    })
    return {
        Accordion: container,
        Admonition: container,
        Avatar: defineComponent({
            props: ['src'],
            setup: (props) => () => h('img', { src: props.src }),
        }),
        Button: defineComponent({
            props: ['disabled'],
            setup:
                (props, { slots }) =>
                () =>
                    h('button', { disabled: props.disabled }, slots.default?.()),
        }),
        Checkbox: defineComponent({
            props: ['modelValue', 'label', 'disabled'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('label', [
                        props.label,
                        h('input', {
                            type: 'checkbox',
                            checked: props.modelValue,
                            disabled: props.disabled,
                            onChange: (event: Event) =>
                                emit(
                                    'update:modelValue',
                                    (event.target as HTMLInputElement).checked,
                                ),
                        }),
                    ]),
        }),
        IconButton: container,
        NewModal: defineComponent({
            props: ['header'],
            setup(props, { slots, expose }) {
                const visible = ref(false)
                expose({
                    show: () => {
                        visible.value = true
                    },
                    hide: () => {
                        visible.value = false
                    },
                })
                return () =>
                    visible.value
                        ? h('section', { 'data-modal': props.header }, slots.default?.())
                        : null
            },
        }),
        StyledInput: defineComponent({
            props: ['modelValue', 'disabled', 'type'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('input', {
                        value: props.modelValue,
                        disabled: props.disabled,
                        type: props.type ?? 'text',
                        onInput: (event: Event) =>
                            emit('update:modelValue', (event.target as HTMLInputElement).value),
                    }),
        }),
        commonMessages: {},
        defineMessages: (messages: unknown) => messages,
        injectNotificationManager: () => ({ handleError: backend.error }),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message?.defaultMessage,
        }),
    }
})

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: Error) => void
    const promise = new Promise<T>((res, rej) => {
        resolve = res
        reject = rej
    })
    return { promise, resolve, reject }
}

function account(name: string) {
    return {
        account_id: 'microsoft:saved-id',
        account_type: 'microsoft',
        profile: {
            id: 'saved-id',
            name,
            skins: [
                { state: 'ACTIVE', url: 'https://example.test/skin', textureKey: 'saved-skin' },
            ],
        },
    }
}

const cleanup: (() => void)[] = []
beforeEach(() => {
    vi.resetAllMocks()
    useNetworkStatus().setBrowserOffline(false)
    useNetworkStatus().markBrowserOnline()
    backend.defaultUser.mockResolvedValue('microsoft:saved-id')
    backend.setDefault.mockResolvedValue(undefined)
    backend.skins.mockResolvedValue([])
    backend.users.mockResolvedValue([])
    backend.savedLogins.mockResolvedValue([])
    backend.rememberedPassword.mockResolvedValue(null)
    backend.savePassword.mockResolvedValue(undefined)
})
afterEach(() =>
    cleanup
        .splice(0)
        .reverse()
        .forEach((stop) => stop()),
)

function mountCard() {
    const wrapper = mount(
        defineComponent({
            setup: () => () => h(Suspense, null, { default: () => h(AccountsCard) }),
        }),
        { global: { directives: { tooltip: {} } } },
    )
    cleanup.push(() => wrapper.unmount())
    return wrapper
}

it('shows saved accounts while online profiles and head rendering are still pending', async () => {
    const online = deferred<ReturnType<typeof account>[]>()
    const heads = deferred<string>()
    backend.users.mockImplementation((_: boolean, cachedOnly: boolean) =>
        cachedOnly ? Promise.resolve([account('SavedPlayer')]) : online.promise,
    )
    backend.heads.mockReturnValue(heads.promise)
    const wrapper = mountCard()
    await flushPromises()
    expect(wrapper.text()).toContain('SavedPlayer')
    expect(wrapper.text()).not.toContain('Not signed in')
    expect(backend.users).toHaveBeenCalledWith(false, true)
    expect(backend.users).toHaveBeenCalledWith(false)
    expect(backend.skins).not.toHaveBeenCalled()

    heads.resolve('data:image/png;base64,cached-avatar')
    await flushPromises()
    expect(wrapper.find('img').attributes('src')).toBe('data:image/png;base64,cached-avatar')
    online.resolve([account('UpdatedPlayer')])
    await flushPromises()
    expect(wrapper.text()).toContain('UpdatedPlayer')
    expect(backend.skins).toHaveBeenCalledOnce()
})

it('keeps local accounts and cached avatars when online refresh fails', async () => {
    const online = deferred<ReturnType<typeof account>[]>()
    backend.users.mockImplementation((_: boolean, cachedOnly: boolean) =>
        cachedOnly ? Promise.resolve([account('SavedPlayer')]) : online.promise,
    )
    backend.heads.mockResolvedValue('data:image/png;base64,cached-avatar')
    const wrapper = mountCard()
    await flushPromises()
    online.reject(new Error('Mojang unavailable'))
    await flushPromises()
    expect(wrapper.text()).toContain('SavedPlayer')
    expect(wrapper.find('img').attributes('src')).toBe('data:image/png;base64,cached-avatar')
    expect(backend.error).toHaveBeenCalledOnce()
})

it('selects a saved account by its provider-qualified account ID', async () => {
    const savedAccount = account('SavedPlayer')
    backend.users.mockResolvedValue([savedAccount])

    const wrapper = mountCard()
    await flushPromises()

    const accountButton = wrapper
        .findAll('button')
        .find((button) => button.text().includes('SavedPlayer'))
    expect(accountButton).toBeDefined()
    await accountButton!.trigger('click')
    await flushPromises()

    expect(backend.setDefault).toHaveBeenCalledWith(savedAccount.account_id)
    expect(backend.setDefault).not.toHaveBeenCalledWith(savedAccount.profile.id)
})

it('does not let a late cached head overwrite the refreshed skin', async () => {
    const online = deferred<ReturnType<typeof account>[]>()
    const oldHead = deferred<string>()
    const newHead = deferred<string>()
    backend.users.mockImplementation((_: boolean, cachedOnly: boolean) =>
        cachedOnly ? Promise.resolve([account('SavedPlayer')]) : online.promise,
    )
    backend.heads.mockImplementation((skin: { texture_key: string }) =>
        skin.texture_key === 'saved-skin' ? oldHead.promise : newHead.promise,
    )
    const wrapper = mountCard()
    await flushPromises()
    const updated = account('UpdatedPlayer')
    updated.profile.skins[0].textureKey = 'updated-skin'
    online.resolve([updated])
    await flushPromises()
    newHead.resolve('data:image/png;base64,new-avatar')
    await flushPromises()
    oldHead.resolve('data:image/png;base64,old-avatar')
    await flushPromises()
    expect(wrapper.text()).toContain('UpdatedPlayer')
    expect(wrapper.find('img').attributes('src')).toBe('data:image/png;base64,new-avatar')
})

it('creates an offline account through the visible form and selects its account ID', async () => {
    const created = {
        account_id: 'offline:local-id',
        account_type: 'offline',
        profile: { id: 'local-id', name: 'Local_Player' },
    }
    let saved: (typeof created)[] = []
    backend.users.mockImplementation(async () => saved)
    backend.offline.mockImplementation(async () => {
        saved = [created]
        return created
    })
    const wrapper = mountCard()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Add offline account')!
        .trigger('click')
    const dialog = wrapper.find('section[data-modal="Add offline account"]')
    await dialog.find('input').setValue('Local_Player')
    await dialog
        .findAll('button')
        .find((button) => button.text() === 'Create account')!
        .trigger('click')
    await flushPromises()
    expect(backend.offline).toHaveBeenCalledWith('Local_Player', undefined)
    expect(backend.setDefault).toHaveBeenCalledWith('offline:local-id')
    expect(wrapper.text()).toContain('Local_Player')
    expect(wrapper.find('section[data-modal="Add offline account"]').exists()).toBe(false)
})

it('opens third-party sign in immediately while the credential store is still pending, then saves a selected profile', async () => {
    const savedLogins = deferred<[]>()
    backend.savedLogins.mockReturnValue(savedLogins.promise)
    backend.beginYggdrasil.mockResolvedValue({
        status: 'select_profile',
        flow_id: 'flow-id',
        profiles: [{ id: 'profile-id', name: 'SkinPlayer' }],
    })
    const created = {
        account_id: 'yggdrasil:https://littleskin.cn/api/yggdrasil:profile-id',
        account_type: 'yggdrasil',
        profile: { id: 'profile-id', name: 'SkinPlayer' },
    }
    backend.finishYggdrasil.mockResolvedValue(created)
    const wrapper = mountCard()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Add third-party account')!
        .trigger('click')
    const dialog = wrapper.find('section[data-modal="Sign in with a third-party service"]')
    expect(dialog.exists()).toBe(true)
    savedLogins.resolve([])
    await flushPromises()
    const inputs = dialog.findAll('input:not([type="checkbox"])')
    await inputs[1].setValue('player@example.test')
    await inputs[2].setValue('test-password')
    await dialog
        .findAll('button')
        .find((button) => button.text() === 'Sign in')!
        .trigger('click')
    await flushPromises()
    expect(backend.beginYggdrasil).toHaveBeenCalledWith(
        'https://littleskin.cn/api/yggdrasil',
        'player@example.test',
        'test-password',
    )
    const profiles = wrapper.find('section[data-modal="Select a profile"]')
    await profiles
        .findAll('button')
        .find((button) => button.text() === 'SkinPlayer')!
        .trigger('click')
    await flushPromises()
    expect(backend.finishYggdrasil).toHaveBeenCalledWith('flow-id', 'profile-id')
    expect(backend.setDefault).toHaveBeenCalledWith(created.account_id)
    expect(backend.savePassword).toHaveBeenCalledWith(
        'https://littleskin.cn/api/yggdrasil',
        'player@example.test',
        'test-password',
    )
})

it('exposes device code login without waiting for the embedded Microsoft window', async () => {
    const wrapper = mountCard()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Use device code')!
        .trigger('click')
    expect(backend.deviceLogin).toHaveBeenCalledOnce()
})
