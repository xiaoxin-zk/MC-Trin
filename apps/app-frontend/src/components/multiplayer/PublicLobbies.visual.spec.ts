import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import type { PublicLobby } from '@/helpers/public-lobbies'

import PublicLobbies from './PublicLobbies.vue'

const fixture = vi.hoisted(() => ({ fetch: vi.fn(), startMctier: vi.fn(), push: vi.fn() }))

vi.mock('@/helpers/public-lobbies', async () => {
    const actual = await vi.importActual<typeof import('@/helpers/public-lobbies')>(
        '@/helpers/public-lobbies',
    )
    return { ...actual, fetchPublicLobbies: fixture.fetch }
})
vi.mock('@/helpers/multiplayer', () => ({
    multiplayer: { startMctier: fixture.startMctier },
    multiplayerErrorText: (error: unknown) => String(error),
    validLocalPort: (value: string) => {
        if (!/^\d+$/.test(value)) return null
        const port = Number(value)
        return Number.isInteger(port) && port >= 1 && port <= 65535 ? port : null
    },
}))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: fixture.push }) }))
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    return {
        Admonition: defineComponent({
            setup:
                (_, { slots }) =>
                () =>
                    h('div', slots.default?.()),
        }),
        Button: defineComponent({
            props: ['disabled', 'icon'],
            setup:
                (props, { slots }) =>
                () =>
                    h('button', { disabled: props.disabled }, slots.default?.()),
        }),
        defineMessages: (messages: unknown) => messages,
        useVIntl: () => ({
            formatMessage: (
                message: { defaultMessage: string },
                values: Record<string, unknown> = {},
            ) =>
                Object.entries(values).reduce(
                    (text, [key, value]) => text.replace(`{${key}}`, String(value)),
                    message.defaultMessage,
                ),
        }),
    }
})

const sampleLobby: PublicLobby = {
    lobbyName: 'Builders',
    hostName: 'Alice',
    playerCount: 2,
    maxPlayers: 8,
    description: 'A friendly building room',
}

let wrappers: VueWrapper[] = []

function mountPage() {
    const wrapper = mount(PublicLobbies)
    wrappers.push(wrapper)
    return wrapper
}

beforeEach(() => {
    vi.resetAllMocks()
    fixture.push.mockResolvedValue(undefined)
    fixture.startMctier.mockResolvedValue(undefined)
})

afterEach(() => {
    wrappers.forEach((wrapper) => wrapper.unmount())
    wrappers = []
})

it('loads public rooms, opens a real join flow, and refreshes the listing', async () => {
    let resolveListing!: (lobbies: PublicLobby[]) => void
    fixture.fetch
        .mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    resolveListing = resolve
                }),
        )
        .mockResolvedValueOnce([])
    const wrapper = mountPage()
    await nextTick()
    expect(wrapper.text()).toContain('Loading public lobbies...')
    resolveListing([sampleLobby])
    await flushPromises()

    expect(wrapper.text()).toContain('Builders')
    expect(wrapper.text()).toContain('Hosted by Alice')
    expect(wrapper.text()).toContain('2 / 8 players')
    expect(wrapper.text()).toContain('Join lobby')

    await wrapper.get('ul button').trigger('click')
    expect(wrapper.find('form').exists()).toBe(true)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(fixture.startMctier).toHaveBeenCalledWith(
        expect.objectContaining({ mode: 'join', lobby_name: 'Builders', game_port: 25565 }),
    )
    expect(fixture.push).toHaveBeenCalledWith({
        path: '/multiplayer/rooms',
        query: { provider: 'mctier' },
    })

    await wrapper.get('header button').trigger('click')
    await flushPromises()
    expect(fixture.fetch).toHaveBeenCalledTimes(2)
    expect(wrapper.text()).toContain('No public lobbies are available right now.')
})

it('shows a recoverable error and the empty state after a successful retry', async () => {
    fixture.fetch.mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce([])
    const wrapper = mountPage()
    await flushPromises()

    expect(wrapper.get('[role="alert"]').text()).toContain('Check your connection')
    await wrapper.get('header button').trigger('click')
    await flushPromises()

    expect(wrapper.text()).toContain('No public lobbies are available right now.')
})

it('disables refresh while loading and cancels the request when leaving', async () => {
    fixture.fetch.mockImplementation(() => new Promise(() => {}))
    const wrapper = mountPage()
    await nextTick()
    expect(wrapper.get('header button').attributes('disabled')).toBeDefined()
    await wrapper.get('header button').trigger('click')
    expect(fixture.fetch).toHaveBeenCalledTimes(1)

    const signal = fixture.fetch.mock.calls[0][1] as AbortSignal
    wrapper.unmount()
    wrappers = []
    expect(signal.aborted).toBe(true)
})

it('validates the Minecraft port before starting a public lobby session', async () => {
    fixture.fetch.mockResolvedValueOnce([sampleLobby])
    const wrapper = mountPage()
    await flushPromises()
    await wrapper.get('ul button').trigger('click')
    await wrapper.findAll('form input')[1].setValue('0')
    await wrapper.get('form').trigger('submit')
    expect(fixture.startMctier).not.toHaveBeenCalled()
    expect(wrapper.get('[role="alert"]').text()).toContain('between 1 and 65535')
})
