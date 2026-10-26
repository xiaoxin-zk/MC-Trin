import { flushPromises, mount } from '@vue/test-utils'
import { beforeEach, expect, it, vi } from 'vitest'
import { computed, ref } from 'vue'

import type { useLogAgent } from '@/composables/useLogAgent'
import { DEFAULT_AI_AGENT_PROFILE } from '@/helpers/ai-agent-profiles'

import AIAgent from './AIAgent.vue'

const mocks = vi.hoisted(() => ({
    instances: vi.fn(),
    catalog: vi.fn(),
    aiState: vi.fn(),
    analysisSettings: vi.fn(),
    handleError: vi.fn(),
    push: vi.fn(),
    mcpTools: [{ name: 'lookup_server', description: 'Read a server status' }],
    mcpToken: '',
    mcpCalls: [] as unknown[][],
    requestLocalToolApproval: false,
    localApprovalResult: null as Promise<boolean> | null,
    agent: undefined as unknown as ReturnType<typeof useLogAgent>,
}))

vi.mock('@/helpers/instance', () => ({ list: mocks.instances }))
vi.mock('@/helpers/ai', () => ({
    getAICatalog: mocks.catalog,
    getAIState: mocks.aiState,
}))
vi.mock('@/helpers/logs', () => ({
    get_crash_analysis_ai_settings: mocks.analysisSettings,
}))
vi.mock('@/composables/useLogAgent', () => ({ useLogAgent: () => mocks.agent }))
vi.mock('@/assistant/mcp-client', () => ({
    StreamableHttpMcpClient: class {
        endpoint: string

        constructor(endpoint: string, token: string) {
            this.endpoint = endpoint
            mocks.mcpToken = token
        }

        async connect() {
            return mocks.mcpTools
        }

        async callTool(name: string, args: Record<string, unknown>) {
            mocks.mcpCalls.push([name, args])
            return { content: [{ type: 'text', text: 'online' }] }
        }
    },
}))
vi.mock('@tauri-apps/plugin-http', () => ({ fetch: vi.fn() }))
vi.mock('@/components/ui/LogAgentActivity.vue', async () => {
    const { defineComponent } = await import('vue')
    return { default: defineComponent({ render: () => null }) }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: mocks.push }) }))
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    return {
        Button: defineComponent({
            inheritAttrs: false,
            props: ['disabled', 'type', 'color'],
            setup(props, { attrs, slots }) {
                return () =>
                    h(
                        'button',
                        { ...attrs, disabled: props.disabled, type: 'button' },
                        slots.default?.(),
                    )
            },
        }),
        defineMessages: (messages: unknown) => messages,
        injectNotificationManager: () => ({ handleError: mocks.handleError }),
        useVIntl: () => ({
            formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
        }),
    }
})

const installedInstance = {
    id: 'instance-1',
    name: 'Survival',
    game_version: '1.21.4',
    install_stage: 'installed',
}

function configuredAI() {
    return {
        settings: { enabled: true },
        providers: [
            {
                provider_id: 'openai',
                enabled: true,
                models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini', enabled: true }],
                has_api_key: true,
                oauth_connected: false,
                configured_credentials: [],
            },
        ],
    }
}

function configuredAnalysis(enabled = true) {
    return {
        enabled,
        provider_id: 'openai',
        model_id: 'gpt-4o-mini',
        ai_source: 'custom',
    }
}

function mountPage() {
    return mount(AIAgent)
}

function latestStartCall(): unknown[] {
    return (
        mocks.agent.start as unknown as {
            mock: { calls: unknown[][] }
        }
    ).mock.calls.at(-1)!
}

beforeEach(() => {
    vi.resetAllMocks()
    localStorage.removeItem('mc-trin:ai-agent-profiles:v1')
    localStorage.removeItem('mctrin.assistant.tool-settings.v1')
    localStorage.removeItem('mctrin.assistant.mcp-servers.v1')
    localStorage.removeItem('mctrin.assistant.mcp-endpoint.v1')
    mocks.instances.mockResolvedValue([installedInstance])
    mocks.catalog.mockResolvedValue([{ id: 'openai', auth_type: 'apiKey' }])
    mocks.aiState.mockResolvedValue(configuredAI())
    mocks.analysisSettings.mockResolvedValue(configuredAnalysis())
    mocks.mcpTools = [{ name: 'lookup_server', description: 'Read a server status' }]
    mocks.mcpToken = ''
    mocks.mcpCalls = []
    mocks.requestLocalToolApproval = false
    mocks.localApprovalResult = null
    mocks.agent = {
        status: ref('idle'),
        output: ref(''),
        error: ref(''),
        event: ref(null),
        busy: computed(() => false),
        runSource: ref(''),
        model: ref(''),
        activity: ref([]),
        elapsed: ref(0),
        start: vi.fn(async (...args: unknown[]) => {
            const remote = args[5] as
                | {
                      tools: Array<{ name: string }>
                      approveTool: (
                          tool: { name: string },
                          args: Record<string, unknown>,
                      ) => Promise<boolean>
                      callTool: (
                          name: string,
                          args: Record<string, unknown>,
                          signal: AbortSignal,
                      ) => Promise<unknown>
                  }
                | undefined
            if (remote) {
                const callArgs = { address: 'play.example.test' }
                const allowed = await remote.approveTool(remote.tools[0], callArgs)
                if (allowed)
                    await remote.callTool(
                        remote.tools[0].name,
                        callArgs,
                        new AbortController().signal,
                    )
            }
            const approveLocalTool = args[8] as
                | ((
                      tool: { name: string; description: string; permission: 'write' },
                      args: Record<string, unknown>,
                  ) => Promise<boolean>)
                | undefined
            if (mocks.requestLocalToolApproval && approveLocalTool) {
                mocks.localApprovalResult = approveLocalTool(
                    {
                        name: 'update_launcher_settings',
                        description: 'Update non-sensitive launcher settings.',
                        permission: 'write',
                    },
                    { patch: { theme: 'dark' } },
                )
            }
        }),
        cancel: vi.fn(),
        reset: vi.fn(),
    } as unknown as ReturnType<typeof useLogAgent>
})

it('routes missing provider configuration to AI settings', async () => {
    mocks.aiState.mockResolvedValue({ settings: { enabled: false }, providers: [] })
    const wrapper = mountPage()
    await flushPromises()

    const configure = wrapper
        .findAll('button')
        .find((button) => button.text().includes('Configure AI'))
    expect(configure).toBeDefined()
    await configure!.trigger('click')
    expect(mocks.push).toHaveBeenCalledWith({ path: '/settings', hash: '#ai' })
})

it('routes disabled local analysis to its settings', async () => {
    mocks.analysisSettings.mockResolvedValue(configuredAnalysis(false))
    const wrapper = mountPage()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Log diagnosis'))!
        .trigger('click')

    const configure = wrapper
        .findAll('button')
        .find((button) => button.text().includes('Configure AI'))
    expect(configure).toBeDefined()
    expect(wrapper.text()).toContain('Enable local log analysis')
    await configure!.trigger('click')
    expect(mocks.push).toHaveBeenCalledWith({ path: '/settings', hash: '#logs' })
})

it('starts a local analysis for the selected instance and question', async () => {
    const wrapper = mountPage()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Log diagnosis'))!
        .trigger('click')
    await wrapper.get('textarea').setValue('Find the earliest error.')
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Analyze logs'))!
        .trigger('click')

    const args = latestStartCall()
    expect(args.slice(0, 3)).toEqual(['instance-1', 'custom', null])
    expect(typeof args[3]).toBe('function')
    expect(args.slice(4)).toEqual([
        'Find the earliest error.',
        undefined,
        undefined,
        '',
        undefined,
        true,
    ])
})

it('answers general questions without an installed instance or enabled log analysis', async () => {
    mocks.instances.mockResolvedValue([])
    mocks.analysisSettings.mockResolvedValue(configuredAnalysis(false))
    const wrapper = mountPage()
    await flushPromises()

    expect(wrapper.find('textarea').exists()).toBe(true)
    expect(wrapper.text()).not.toContain('Create an instance before analyzing its logs.')
    await wrapper.get('textarea').setValue('How do I change the game memory limit?')
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Ask assistant'))!
        .trigger('click')
    const args = latestStartCall()
    expect(args.slice(0, 3)).toEqual(['', 'assistant', null])
    expect(typeof args[3]).toBe('function')
    expect(args.slice(4, 8)).toEqual([
        'How do I change the game memory limit?',
        undefined,
        { providerId: 'openai', modelId: 'gpt-4o-mini' },
        DEFAULT_AI_AGENT_PROFILE.instructions,
    ])
    expect(typeof args[8]).toBe('function')
    expect(args[9]).toBe(true)
})

it('keeps tool settings collapsed and shows a visible reset result', async () => {
    localStorage.setItem(
        'mctrin.assistant.tool-settings.v1',
        JSON.stringify({ disabled: ['get_instance_overview'] }),
    )
    const wrapper = mountPage()
    await flushPromises()

    const tools = wrapper.get('details.assistant-tools')
    expect(tools.attributes('open')).toBeUndefined()
    expect(tools.text()).toMatch(/\d+\/\d+/)
    await tools
        .findAll('button')
        .find((button) => button.text().includes('Enable all'))!
        .trigger('click')
    expect(tools.get('[role="status"]').exists()).toBe(true)
    expect(tools.text()).toMatch(/\d+\/\d+/)
})

it('creates a named agent profile and requires approval for assistant write tools', async () => {
    const wrapper = mountPage()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('New agent'))!
        .trigger('click')
    await wrapper.get('[role="dialog"] input').setValue('Modpack helper')
    await wrapper
        .get('[role="dialog"] textarea')
        .setValue('Focus on loader and dependency compatibility.')
    await wrapper.get('[role="dialog"]').trigger('submit')

    expect(wrapper.text()).toContain('Modpack helper')
    mocks.requestLocalToolApproval = true
    await wrapper.get('textarea').setValue('Why does this pack fail to launch?')
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Ask assistant'))!
        .trigger('click')

    const args = latestStartCall()
    expect(args.slice(0, 3)).toEqual(['instance-1', 'assistant', null])
    expect(typeof args[3]).toBe('function')
    expect(args.slice(4, 8)).toEqual([
        'Why does this pack fail to launch?',
        undefined,
        { providerId: 'openai', modelId: 'gpt-4o-mini' },
        'Focus on loader and dependency compatibility.',
    ])
    expect(typeof args[8]).toBe('function')
    expect(args[9]).toBe(true)

    await flushPromises()
    const approval = wrapper.get('[role="alertdialog"]')
    expect(approval.text()).toContain('Review local write request')
    expect(approval.text()).toContain('MC-Trin')
    expect(approval.text()).toContain('update_launcher_settings')
    expect(approval.text()).toContain('Update non-sensitive launcher settings.')
    expect(approval.text()).toContain('"theme": "dark"')
    expect(mocks.localApprovalResult).not.toBeNull()
    await wrapper
        .findAll('button')
        .find((button) => button.text().trim() === 'Deny')!
        .trigger('click')
    expect(await mocks.localApprovalResult).toBe(false)
})

it('offers instance creation only when log diagnosis needs an instance', async () => {
    mocks.instances.mockResolvedValue([])
    const wrapper = mountPage()
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Log diagnosis'))!
        .trigger('click')

    expect(wrapper.text()).toContain('Create an instance before analyzing its logs.')
    expect(wrapper.find('textarea').exists()).toBe(false)
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Create an instance'))!
        .trigger('click')
    expect(mocks.push).toHaveBeenCalledWith('/create')
})

it('shows the MCP endpoint, tool, and full arguments before approving a remote call', async () => {
    const wrapper = mountPage()
    await flushPromises()
    await wrapper.find('input[type="text"]').setValue('Community tools')
    await wrapper.find('input[type="url"]').setValue('https://mcp.example.test/mcp')
    await wrapper.find('input[type="password"]').setValue('secret-token')
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Save server'))!
        .trigger('click')
    const savedServers = localStorage.getItem('mctrin.assistant.mcp-servers.v1') ?? ''
    expect(savedServers).toContain('Community tools')
    expect(savedServers).toContain('https://mcp.example.test/mcp')
    expect(savedServers).not.toContain('secret-token')
    await wrapper
        .findAll('button')
        .find((button) => button.text().trim() === 'Connect')!
        .trigger('click')
    await flushPromises()

    expect(mocks.mcpToken).toBe('secret-token')
    expect((wrapper.find('input[type="password"]').element as HTMLInputElement).value).toBe('')
    expect(localStorage.getItem('mctrin.assistant.mcp-endpoint.v1')).toBe(
        'https://mcp.example.test/mcp',
    )

    await wrapper.find('textarea').setValue('Is this server online?')
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Ask assistant'))!
        .trigger('click')
    await flushPromises()
    expect(wrapper.get('[role="alertdialog"]').text()).toContain('https://mcp.example.test/mcp')
    expect(wrapper.get('[role="alertdialog"]').text()).toContain('lookup_server')
    expect(wrapper.get('[role="alertdialog"]').text()).toContain('play.example.test')

    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Deny'))!
        .trigger('click')
    await flushPromises()
    expect(mocks.mcpCalls).toEqual([])

    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Ask assistant'))!
        .trigger('click')
    await flushPromises()
    await wrapper
        .findAll('button')
        .find((button) => button.text().includes('Allow this call'))!
        .trigger('click')
    await flushPromises()
    expect(mocks.mcpCalls).toEqual([['lookup_server', { address: 'play.example.test' }]])
})
