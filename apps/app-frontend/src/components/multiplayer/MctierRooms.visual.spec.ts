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
                    maxlength: props.maxlength,
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
        ProgressBar: defineComponent({
            props: ['progress', 'max', 'waiting', 'label', 'fullWidth', 'showProgress'],
            setup: (props) => () =>
                h(
                    'div',
                    {
                        role: 'progressbar',
                        'data-waiting': props.waiting ? 'true' : 'false',
                        'aria-valuenow': props.waiting ? undefined : String(props.progress),
                    },
                    props.label ? String(props.label) : undefined,
                ),
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
    publish_outcome: 'private' as const,
    max_players: null,
    has_password: false,
    download_stage: null,
    download_progress: null,
    host_hint: null,
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

function button(wrapper: VueWrapper, text: string) {
    const found = wrapper.findAll('button').find((candidate) => candidate.text() === text)
    if (!found) throw new Error(`No button labelled "${text}"`)
    return found
}

const voicePanel = '[data-onboarding-id="mctier-voice"]'
/** The bordered card that only makes sense while a call is up. */
const voiceSessionCard = `${voicePanel} > div.rounded-lg.border`

it('validates fields and starts a host session with the expected DTO', async () => {
    const wrapper = page()
    await button(wrapper, 'Create lobby').trigger('click')
    expect(fixture.start).not.toHaveBeenCalled()

    await fillRequired(wrapper)
    await button(wrapper, 'Create lobby').trigger('click')
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
        description: null,
        max_players: null,
    })
})

it('explains why Create is disabled instead of silently ignoring the click', async () => {
    const wrapper = page()
    const createForm = '[data-onboarding-id="mctier-create-form"]'

    // (a) A fresh form has no lobby name. Create is disabled, and the form says
    // which field is missing rather than leaving a dead button with no message.
    const blocked = wrapper.get(`${createForm} [role="status"]`)
    expect(blocked.text()).toContain('Enter a lobby name so friends can find the room')
    expect(button(wrapper, 'Create lobby').attributes('disabled')).toBeDefined()

    // The reason carries its own fix: one click fills a valid generated name.
    await wrapper.get(`${createForm} [role="status"] button`).trigger('click')
    await flushPromises()

    // (b) The generated name is valid, so the reason disappears and Create is live.
    const name = wrapper.get('input[name="lobby-name"]').element as HTMLInputElement
    expect(name.value).toMatch(/^mc-[a-z0-9]{6}$/u)
    expect(wrapper.find(`${createForm} [role="status"]`).exists()).toBe(false)
    expect(button(wrapper, 'Create lobby').attributes('disabled')).toBeUndefined()

    // (c) A typed name that breaks the 4-32 character rule shows its own reason.
    await wrapper.get('input[name="lobby-name"]').setValue('ab')
    await flushPromises()

    const invalid = wrapper.get(`${createForm} [role="status"]`)
    expect(invalid.text()).toContain(
        'Lobby names use 4-32 letters, numbers, underscores, or hyphens.',
    )
    expect(button(wrapper, 'Create lobby').attributes('disabled')).toBeDefined()

    // (d) Symbol-only names are 4 characters long but have no letter or number.
    // The backend rejects them, so the form must refuse them with the same
    // reason instead of letting the request fail after the kernel starts.
    await wrapper.get('input[name="lobby-name"]').setValue('____')
    await flushPromises()

    const symbolOnly = wrapper.get(`${createForm} [role="status"]`)
    expect(symbolOnly.text()).toContain(
        'Lobby names use 4-32 letters, numbers, underscores, or hyphens.',
    )
    expect(button(wrapper, 'Create lobby').attributes('disabled')).toBeDefined()
})

it('uses Unicode code-point limits without truncating names by UTF-16 units', async () => {
    const wrapper = page()
    const lobbyName = wrapper.get('input[name="lobby-name"]')
    const playerName = wrapper.get('input[name="player-name"]')

    expect((lobbyName.element as HTMLInputElement).maxLength).toBe(-1)
    expect((playerName.element as HTMLInputElement).maxLength).toBe(-1)
    await lobbyName.setValue('𝟘'.repeat(32))
    await playerName.setValue('😀'.repeat(64))
    await flushPromises()

    expect([...(lobbyName.element as HTMLInputElement).value]).toHaveLength(32)
    expect([...(playerName.element as HTMLInputElement).value]).toHaveLength(64)
    expect(button(wrapper, 'Create lobby').attributes('disabled')).toBeUndefined()
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
    await button(wrapper, 'Leave lobby').trigger('click')
    expect(fixture.stop).toHaveBeenCalledOnce()
})

it('cancels an in-progress network operation', async () => {
    const wrapper = page({ ...idle, status: 'connecting' })
    await button(wrapper, 'Cancel connection').trigger('click')
    expect(fixture.stop).toHaveBeenCalledOnce()
})

it('shows preparation progress while the network component downloads', () => {
    const wrapper = page({
        ...idle,
        status: 'downloading',
        download_stage: 'downloading',
        download_progress: 42,
    })
    const bar = wrapper.get('[role="progressbar"]')
    expect(bar.attributes('aria-valuenow')).toBe('42')
    expect(bar.attributes('data-waiting')).toBe('false')
    expect(wrapper.text()).toContain('Downloading the networking component…')
})

it('warns the host while Minecraft is not listening on the game port', () => {
    const wrapper = page({
        ...idle,
        status: 'hosting',
        game_port: 25565,
        host_hint: 'game_port_closed',
    })
    expect(wrapper.text()).toContain('Minecraft is not listening on port 25565 yet')
})

it('shows an indeterminate bar when the download size is unknown', () => {
    const wrapper = page({
        ...idle,
        status: 'downloading',
        download_stage: 'extracting',
        download_progress: null,
    })
    const bar = wrapper.get('[role="progressbar"]')
    expect(bar.attributes('data-waiting')).toBe('true')
    expect(wrapper.text()).toContain('Extracting the networking component…')
})

it('keeps public rooms passwordless and sends the chosen visibility', async () => {
    const wrapper = page()
    await fillRequired(wrapper)
    await wrapper.get('input[name="lobby-password"]').setValue('')
    await wrapper.get('input[name="public-lobby"]').setValue(true)
    await button(wrapper, 'Create lobby').trigger('click')
    await flushPromises()
    expect(fixture.start).toHaveBeenCalledWith(
        expect.objectContaining({
            mode: 'create',
            is_public: true,
            password: '',
        }),
    )
})

it('clears the password when the room is listed publicly', async () => {
    const wrapper = page()
    await fillRequired(wrapper)
    // The directory cannot carry a password, so choosing to publish wipes the
    // secret instead of sending a combination the service ignores.
    await wrapper.get('input[name="lobby-password"]').setValue('secret')
    await wrapper.get('input[name="public-lobby"]').setValue(true)
    await flushPromises()
    expect((wrapper.get('input[name="lobby-password"]').element as HTMLInputElement).value).toBe('')
    await button(wrapper, 'Create lobby').trigger('click')
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
    await wrapper.find('form:last-of-type').trigger('submit')
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

    await wrapper
        .get('[data-onboarding-id="mctier-publish-state"] input[type="checkbox"]')
        .setValue(true)
    expect(fixture.setPublic).toHaveBeenCalledWith(true)
})

it('offers the voice panel while a room is still being created', async () => {
    const wrapper = page({ ...idle, status: 'connecting' })
    await flushPromises()

    expect(wrapper.find(voicePanel).exists()).toBe(true)
    expect(wrapper.text()).toContain('Room voice')
    expect(wrapper.find(`${voicePanel} select`).exists()).toBe(true)
    // Device selection is the point of the pre-connect panel, so it must not be
    // wrapped in the session card that has nothing to show yet.
    expect(wrapper.find(voiceSessionCard).exists()).toBe(false)
    // Nothing to signal from yet, so the call itself stays disabled.
    expect(button(wrapper, 'Enable voice').attributes('disabled')).toBeDefined()
})

it('offers a working voice panel once the room is connected', async () => {
    const wrapper = page({
        ...idle,
        status: 'hosting',
        mode: 'create',
        lobby_name: 'TestRoom',
        local_player_id: 'a'.repeat(64),
    })
    await flushPromises()

    expect(wrapper.find(voicePanel).exists()).toBe(true)
    expect(wrapper.text()).toContain('Room voice')
    // Open-mic is the default: no activation checkbox is forced on.
    expect(wrapper.text()).not.toContain('Only send while I speak')
    expect(button(wrapper, 'Enable voice').attributes('disabled')).toBeUndefined()
})

it('keeps the voice panel reachable from the idle lobby form', async () => {
    const wrapper = page()
    await flushPromises()

    expect(wrapper.find(voicePanel).exists()).toBe(true)
    expect(wrapper.find('input[name="lobby-name"]').exists()).toBe(true)
    expect(button(wrapper, 'Enable voice').attributes('disabled')).toBeDefined()
})
