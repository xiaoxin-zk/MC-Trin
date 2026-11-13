import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import type { HongshiState, MultiplayerState } from '@/helpers/multiplayer'

import MultiplayerRooms from './MultiplayerRooms.vue'

const fixture = vi.hoisted(() => ({
    state: null as MultiplayerState | null,
    query: {} as Record<string, string>,
    ports: [] as unknown[],
    host: vi.fn(),
    stop: vi.fn(),
    switchProvider: vi.fn(),
    notify: vi.fn(),
    push: vi.fn(),
}))

vi.mock('@/helpers/multiplayer', async (original) => ({
    ...(await original<typeof import('@/helpers/multiplayer')>()),
    multiplayer: {
        getState: async () => structuredClone(fixture.state),
        getNodes: async () => [
            {
                name: 'New region',
                address: 'relay.example.com',
                latency_ms: 1,
                reachable: true,
                cached: false,
            },
        ],
        getDetectedPorts: async () => structuredClone(fixture.ports),
        getPlayerName: async () => 'Player',
        hostHongshi: fixture.host,
        stop: fixture.stop,
        switchProvider: fixture.switchProvider,
        startMctier: vi.fn(),
        stopMctier: vi.fn(),
        createMctierInvite: vi.fn(),
        setMctierPublic: vi.fn(),
        sendMctierVoiceSignal: vi.fn(),
    },
}))
vi.mock('@/helpers/terracotta', () => ({
    isValidTerracottaRoomCode: () => true,
    terracotta: { getPlatformKey: async () => 'macos_arm64', checkForUpdate: async () => null },
}))
vi.mock('@/helpers/utils', () => ({ exportErrorLogs: vi.fn(), openPath: vi.fn() }))
vi.mock('vue-router', () => ({
    useRoute: () => ({ params: { id: 'test-server' }, query: fixture.query }),
    useRouter: () => ({ push: fixture.push, replace: vi.fn() }),
}))

vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    const { default: ProgressBar } =
        await import('../../../../../packages/ui/src/components/base/ProgressBar.vue')
    const container = defineComponent({
        setup:
            (_, { slots }) =>
            () =>
                h('div', slots.default?.()),
    })
    return {
        ...Object.fromEntries(
            [
                'Admonition',
                'Card',
                'CopyCode',
                'NavTabs',
                'PopoutMenu',
                'OverflowMenu',
                'TagItem',
            ].map((name) => [name, container]),
        ),
        NavTabs: defineComponent({
            props: ['links'],
            setup:
                (props, { emit }) =>
                () =>
                    h(
                        'nav',
                        props.links.map((link: { label: string }, index: number) =>
                            h('button', { onClick: () => emit('tab-click', index) }, link.label),
                        ),
                    ),
        }),
        Button: defineComponent({
            props: ['disabled'],
            setup:
                (props, { slots }) =>
                () =>
                    h('button', { disabled: props.disabled }, slots.default?.()),
        }),
        StyledInput: defineComponent({
            props: ['modelValue'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('input', {
                        value: props.modelValue,
                        onInput: (event: Event) =>
                            emit('update:modelValue', (event.target as HTMLInputElement).value),
                    }),
        }),
        // The real primitive is a `role="checkbox"` button (no native input),
        // so the MC-Trin voice panel needs it stubbed to render here.
        Checkbox: defineComponent({
            props: ['modelValue', 'disabled', 'description', 'label'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('button', {
                        type: 'button',
                        role: 'checkbox',
                        'aria-checked': props.modelValue ? 'true' : 'false',
                        disabled: props.disabled,
                        onClick: () => emit('update:modelValue', !props.modelValue),
                    }),
        }),
        // The real primitive is a `role="switch"` button, so it must not render a
        // <select>: the MC-Trin select-count assertion below counts every select
        // on the page and expects them all to belong to the voice panel.
        Toggle: defineComponent({
            props: ['modelValue', 'disabled', 'id'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h('button', {
                        type: 'button',
                        role: 'switch',
                        'aria-checked': props.modelValue ? 'true' : 'false',
                        disabled: props.disabled,
                        onClick: () => emit('update:modelValue', !props.modelValue),
                    }),
        }),
        // Slider renders a real range input inside a boxed track. The MC-Trin
        // spec uses `input[type="range"]` to tell whether session-only controls
        // appeared, so this mock has to keep rendering an actual range input.
        Slider: defineComponent({
            props: ['modelValue', 'min', 'max', 'step', 'unit', 'disabled', 'id'],
            emits: ['update:modelValue'],
            setup: (props) => () =>
                h('input', {
                    type: 'range',
                    min: props.min,
                    max: props.max,
                    step: props.step,
                    value: props.modelValue,
                    disabled: props.disabled,
                }),
        }),
        Combobox: defineComponent({
            props: ['modelValue', 'options'],
            emits: ['update:modelValue'],
            setup:
                (props, { emit }) =>
                () =>
                    h(
                        'select',
                        {
                            value: props.modelValue,
                            onChange: (event: Event) =>
                                emit(
                                    'update:modelValue',
                                    (event.target as HTMLSelectElement).value,
                                ),
                        },
                        props.options.map((option: { value: string; label: string }) =>
                            h('option', { value: option.value }, option.label),
                        ),
                    ),
        }),
        ProgressBar,
        defineMessages: (messages: unknown) => messages,
        injectNotificationManager: () => ({
            handleError: fixture.notify,
            addNotification: fixture.notify,
        }),
        injectFilePicker: () => ({}),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})

let wrappers: VueWrapper[] = []
const idle = (): HongshiState => ({
    supported: true,
    status: 'idle',
    local_port: null,
    node: null,
    public_address: null,
    created_at: null,
    last_exit_code: null,
    error_type: null,
    error_message: null,
    bound_instance_id: null,
    port_changed: false,
    binary_installed: false,
    download_progress: null,
})
async function rooms() {
    const wrapper = mount(MultiplayerRooms, { global: { directives: { tooltip: () => {} } } })
    wrappers.push(wrapper)
    await flushPromises()
    return wrapper
}
beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
    localStorage.setItem('mctrin-multiplayer-provider', 'mctier')
    fixture.query = {}
    fixture.ports = []
    fixture.state = {
        active_provider: null,
        providers: [],
        terracotta: { status: 'idle' },
        hongshi: idle(),
        mctier: {
            status: 'idle',
            mode: null,
            lobby_name: null,
            virtual_ip: null,
            game_port: null,
            connect_address: null,
            players: [],
            error_message: null,
            is_public: false,
            has_password: false,
        },
    } as unknown as MultiplayerState
    fixture.push.mockResolvedValue(undefined)
    fixture.switchProvider.mockImplementation(async () => {
        fixture.state!.active_provider = null
    })
})
afterEach(() => {
    wrappers.forEach((wrapper) => wrapper.unmount())
    wrappers = []
})

it('renders the MC-Trin room workflow by default', async () => {
    const wrapper = await rooms()
    expect(wrapper.text()).toContain('MC-Trin LAN lobby')
    expect(wrapper.text()).not.toContain('RedStone Online 2')
    // Identify the branch structurally instead of by a raw select count: the
    // MC-Trin tab legitimately owns selectors (audio device pickers), so a
    // "no selects" rule would be wrong the moment those render.
    expect(wrapper.find('[data-onboarding-id="mctier-create-form"]').exists()).toBe(true)
    const voicePanel = wrapper.find('[data-onboarding-id="mctier-voice"]')
    expect(voicePanel.exists()).toBe(true)

    const panelSelects = voicePanel.findAll('select')
    expect(panelSelects.length).toBeGreaterThan(0)
    // Every select on the page has to belong to that panel. Anything else would
    // be a legacy provider control (Hongshi's port and node pickers), which is
    // exactly what this test is guarding against. Counted, not hard-coded: the
    // microphone picker is always present, while the speaker picker only appears
    // when the browser reports output devices.
    expect(wrapper.findAll('select')).toHaveLength(panelSelects.length)

    // Kept as a readable statement of what "the legacy workflow did not render"
    // means; no other branch defines these labels.
    expect(wrapper.text()).not.toContain('Local Minecraft port')
    expect(wrapper.text()).not.toContain('Relay node')
})
