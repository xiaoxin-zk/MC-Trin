import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import MctierRooms from './MctierRooms.vue'

const fixture = vi.hoisted(() => ({
    start: vi.fn(),
    stop: vi.fn(),
    setPublic: vi.fn(),
    createInvite: vi.fn(),
    sendVoice: vi.fn(),
}))

vi.mock('@/helpers/multiplayer', async (original) => ({
    ...(await original<typeof import('@/helpers/multiplayer')>()),
    multiplayer: {
        startMctier: fixture.start,
        stop: fixture.stop,
        setMctierPublic: fixture.setPublic,
        createMctierInvite: fixture.createInvite,
        sendMctierVoiceSignal: fixture.sendVoice,
    },
}))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    const input = defineComponent({
        props: ['modelValue', 'name', 'type', 'maxlength', 'inputmode'],
        emits: ['update:modelValue'],
        setup:
            (props, { emit }) =>
            () =>
                h('input', {
                    name: props.name,
                    type: props.type ?? 'text',
                    value: props.modelValue,
                    onInput: (event: Event) =>
                        emit('update:modelValue', (event.target as HTMLInputElement).value),
                }),
    })
    return {
        Button: defineComponent({
            props: ['disabled'],
            setup:
                (props, { slots }) =>
                () =>
                    h('button', { disabled: props.disabled }, slots.default?.()),
        }),
        CopyCode: defineComponent({
            props: ['text'],
            setup: (props) => () => h('code', props.text),
        }),
        StyledInput: input,
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

const idle = {
    status: 'idle' as const,
    mode: null,
    lobby_name: null,
    virtual_ip: null,
    game_port: null,
    connect_address: null,
    players: [],
    local_player_id: null,
    voice_ice_server: null,
    error_message: null,
    is_public: false,
    has_password: false,
}

let wrappers: VueWrapper[] = []

beforeEach(() => {
    vi.clearAllMocks()
    fixture.start.mockResolvedValue(undefined)
    fixture.stop.mockResolvedValue(undefined)
    fixture.setPublic.mockResolvedValue(undefined)
    fixture.createInvite.mockResolvedValue('mctier://join?v=3&name=TestRoom&secret=')
})

afterEach(() => {
    wrappers.forEach((wrapper) => wrapper.unmount())
    wrappers = []
})

function page(state = idle) {
    const wrapper = mount(MctierRooms, { props: { state } })
    wrappers.push(wrapper)
    return wrapper
}

function fillRequired(wrapper: VueWrapper) {
    const values: Record<string, string> = {
        'lobby-name': 'TestRoom',
        'player-name': 'Alice',
        'lobby-password': 'secret',
        'game-port': '25565',
        'server-node': 'tcp://127.0.0.1:11010',
        'signaling-server': 'ws://127.0.0.1:12345',
    }
    return Promise.all(
        Object.entries(values).map(([name, value]) =>
            wrapper.get(`input[name="${name}"]`).setValue(value),
        ),
    )
}

it('validates fields and starts a host session with the expected DTO', async () => {
    const wrapper = page()
    await wrapper.get('button').trigger('click')
    expect(fixture.start).not.toHaveBeenCalled()

    await fillRequired(wrapper)
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Create lobby')!
        .trigger('click')
    await flushPromises()
    expect(fixture.start).toHaveBeenCalledWith({
        mode: 'create',
        lobby_name: 'TestRoom',
        password: 'secret',
        player_name: 'Alice',
        server_node: 'tcp://127.0.0.1:11010',
        signaling_server: 'ws://127.0.0.1:12345',
        game_port: 25565,
        is_public: false,
    })
})

it('shows the verified address and leaves a running session', async () => {
    const wrapper = page({
        ...idle,
        status: 'joined',
        mode: 'join',
        lobby_name: 'TestRoom',
        virtual_ip: '10.126.1.2',
        game_port: 25565,
        connect_address: '127.0.0.1:25566',
        players: [
            { playerId: 'a', playerName: 'Alice', virtualIp: '10.126.1.2', sessionGeneration: 1 },
        ],
    })
    expect(wrapper.text()).toContain('Minecraft connection verified')
    expect(wrapper.text()).toContain('127.0.0.1:25566')
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Leave lobby')!
        .trigger('click')
    expect(fixture.stop).toHaveBeenCalledOnce()
})

it('cancels an in-progress network operation', async () => {
    const wrapper = page({ ...idle, status: 'connecting' })
    await wrapper.get('button').trigger('click')
    expect(fixture.stop).toHaveBeenCalledOnce()
})

it('keeps public rooms passwordless and sends the chosen visibility', async () => {
    const wrapper = page()
    await fillRequired(wrapper)
    await wrapper.get('input[name="lobby-password"]').setValue('')
    await wrapper.get('input[name="public-lobby"]').setValue(true)
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Create lobby')!
        .trigger('click')
    await flushPromises()
    expect(fixture.start).toHaveBeenCalledWith(
        expect.objectContaining({
            mode: 'create',
            is_public: true,
            password: '',
        }),
    )
})

it('parses and joins a MCTier v3 invite while retaining the manual game port', async () => {
    const wrapper = page()
    await wrapper.get('input[name="player-name"]').setValue('Alice')
    await wrapper.get('input[name="game-port"]').setValue('25570')
    await wrapper
        .get('input[name="mctier-invite-code"]')
        .setValue(
            'mctier://join?v=3&name=TestRoom&secret=mctier-invite-v3%3AQUJD&node=tcp%3A%2F%2F127.0.0.1%3A11010&signal=ws%3A%2F%2F127.0.0.1%3A12345',
        )
    await wrapper
        .findAll('button')
        .find((button) => button.text() === 'Join using invite code')!
        .trigger('click')
    await flushPromises()
    expect(fixture.start).toHaveBeenCalledWith(
        expect.objectContaining({
            mode: 'join',
            lobby_name: 'TestRoom',
            password: 'mctier-invite-v3:QUJD',
            game_port: 25570,
        }),
    )
})

it('shows a generated host invite and sends visibility changes to the backend', async () => {
    const inviteCode = 'mctier://join?v=3&name=TestRoom&secret='
    fixture.createInvite.mockResolvedValue(inviteCode)
    const wrapper = page({
        ...idle,
        status: 'hosting',
        mode: 'create',
        lobby_name: 'TestRoom',
    })
    await flushPromises()
    expect(wrapper.text()).toContain('Invite code')
    expect(wrapper.text()).toContain(inviteCode)

    await wrapper.get('input[type="checkbox"]').setValue(true)
    expect(fixture.setPublic).toHaveBeenCalledWith(true)
})

it('shows voice controls only after the lobby session is active', async () => {
    const idleWrapper = page()
    expect(idleWrapper.text()).not.toContain('Enable voice')

    const activeWrapper = page({
        ...idle,
        status: 'hosting',
        mode: 'create',
        local_player_id: 'a'.repeat(64),
    })
    await flushPromises()
    expect(activeWrapper.text()).toContain('Room voice')
    expect(activeWrapper.text()).toContain('Enable voice')
})
