import assert from 'node:assert/strict'
import { test } from 'node:test'

import { AIMessage, ToolMessage } from '@langchain/core/messages'

import { diagnosticText, runLocalAgent } from './agent.ts'
import { LogAgentModel } from './model.ts'
import type { AgentGateway, ModelTurn } from './types.ts'

const session = {
    run_id: 'test',
    provider_id: 'test',
    model_id: 'test',
    locale: 'zh-CN',
    context: 'A crash occurred.',
}

test('executes three grep calls from the first model turn before requesting a diagnosis', async () => {
    let turns = 0
    const queries: string[] = []
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: ['error', 'exception', 'caused by'].map((query, index) => ({
                            id: `grep-${index}`,
                            name: 'grep_log_file',
                            args: { file_id: 'log-1', query },
                        })),
                    }
                assert.equal(turn.messages.filter((message) => message.role === 'tool').length, 3)
                return { role: 'assistant', content: 'Diagnosis after three searches.' }
            },
            tool: async (request) => {
                queries.push(request.query!)
                return 'evidence'
            },
        },
        new AbortController().signal,
        () => {},
    )
    assert.deepEqual(queries.sort(), ['caused by', 'error', 'exception'])
    assert.equal(turns, 2)
    assert.match(output, /Diagnosis/)
})

test("passes the user's question with the prepared log context", async () => {
    let userMessage = ''
    await runLocalAgent(
        session,
        {
            modelTurn: async (turn) => {
                userMessage = turn.messages.map((message) => message.content).join('\n')
                return { role: 'assistant', content: 'The log shows a missing dependency.' }
            },
            tool: async () => null,
        },
        new AbortController().signal,
        () => {},
        undefined,
        '  Find the first error and explain it.  ',
    )

    assert.match(userMessage, /User language: zh-CN/)
    assert.match(userMessage, /A crash occurred\./)
    assert.match(userMessage, /User request: Find the first error and explain it\./)
})

test('assistant mode exposes bounded troubleshooting tools', async () => {
    let turns = 0
    const requests: string[] = []
    const output = await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async (turn) => {
                const names = turn.tools.map((entry) => (entry.function as { name: string }).name)
                assert.deepEqual(names, [
                    'get_instance_overview',
                    'get_launcher_settings',
                    'get_platform_diagnostics',
                    'discover_java_installations',
                    'list_java_vendors',
                    'inspect_instance_content',
                    'check_content_updates',
                    'search_resources',
                    'get_resource_releases',
                    'prepare_resource_install',
                    'prepare_resource_update',
                    'get_install_job',
                    'list_log_files',
                    'read_log_file',
                    'grep_log_file',
                    'update_launcher_settings',
                    'create_instance',
                    'rename_instance',
                    'delete_instance',
                    'install_java_runtime',
                    'set_java_default_runtime',
                    'cancel_install_job',
                    'queue_resource_install',
                    'disable_content',
                    'enable_content',
                    'remove_content',
                ])
                const updateSettings = turn.tools.find(
                    (entry) => entry.function.name === 'update_launcher_settings',
                )
                assert.ok(updateSettings)
                const parameters = updateSettings.function.parameters as {
                    properties: { patch: { minProperties?: number; maxProperties?: number } }
                }
                assert.equal(parameters.properties.patch.minProperties, 1)
                assert.equal(parameters.properties.patch.maxProperties, 8)
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'settings-1',
                                name: 'get_launcher_settings',
                                args: {},
                            },
                        ],
                    }
                assert.match(
                    turn.messages.find((message) => message.role === 'tool')?.content ?? '',
                    /maximum_megabytes/,
                )
                return { role: 'assistant', content: 'The launcher is configured for 4096 MB.' }
            },
            tool: async (request) => {
                requests.push(request.name)
                return { memory: { maximum_megabytes: 4096 } }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        'How much memory is configured?',
        undefined,
        'assistant',
    )

    assert.equal(output, 'The launcher is configured for 4096 MB.')
    assert.deepEqual(requests, ['get_launcher_settings'])
})

test('requires approval before calling an MCP tool and sends the declared schema to the model', async () => {
    let turns = 0
    let approvals = 0
    let calls = 0
    const toolDefinition = {
        name: 'lookup_server',
        description: 'Read a server status',
        inputSchema: {
            type: 'object',
            properties: { address: { type: 'string' } },
            required: ['address'],
        },
    }
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async (turn) => {
                if (++turns === 1) {
                    const remoteTool = turn.tools.find((entry) =>
                        (entry.function as { name: string }).name.startsWith('mcp_0_lookup_server'),
                    )
                    assert.deepEqual(
                        (remoteTool?.function as { parameters: Record<string, unknown> })
                            .parameters['required'],
                        ['address'],
                    )
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'remote-1',
                                name: 'mcp_0_lookup_server',
                                args: { address: 'play.example.test' },
                            },
                        ],
                    }
                }
                const result = turn.messages.find((message) => message.role === 'tool')
                assert.match(result?.content ?? '', /did not approve/)
                return { role: 'assistant', content: 'I did not contact the server.' }
            },
            tool: async () => {
                throw new Error('Local tool was not requested')
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        '',
        {
            endpoint: 'https://mcp.example.test/mcp',
            tools: [toolDefinition],
            approveTool: async (tool, args) => {
                approvals++
                assert.equal(tool.name, 'lookup_server')
                assert.deepEqual(args, { address: 'play.example.test' })
                return false
            },
            callTool: async () => {
                calls++
                return { ok: true }
            },
        },
    )

    assert.equal(approvals, 1)
    assert.equal(calls, 0)
    assert.equal(turns, 2)
    assert.match(output, /did not contact/)
})

test('returns an MCP result to the model after the user approves its exact arguments', async () => {
    let turns = 0
    let approvals = 0
    let calls = 0
    const args = { address: 'play.example.test' }
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'remote-1',
                                name: 'mcp_0_lookup_server',
                                args,
                            },
                        ],
                    }
                assert.match(
                    turn.messages.find((message) => message.role === 'tool')?.content ?? '',
                    /online/,
                )
                return { role: 'assistant', content: 'The server is online.' }
            },
            tool: async () => null,
        },
        new AbortController().signal,
        () => {},
        undefined,
        '',
        {
            endpoint: 'https://mcp.example.test/mcp',
            tools: [{ name: 'lookup_server', inputSchema: { type: 'object' } }],
            approveTool: async (_tool, request) => {
                approvals++
                assert.deepEqual(request, args)
                return true
            },
            callTool: async (name, request) => {
                calls++
                assert.equal(name, 'lookup_server')
                assert.deepEqual(request, args)
                return { status: 'online' }
            },
        },
    )

    assert.equal(approvals, 1)
    assert.equal(calls, 1)
    assert.equal(turns, 2)
    assert.match(output, /server is online/)
})

test('forwards live text and factual tool progress while keeping private protocol data hidden', async () => {
    const events: import('./types.ts').LocalProgress[] = []
    let turns = 0
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async (_turn, receive) => {
                if (++turns === 1) {
                    receive?.({ event_type: 'thinking', data: { content: '' } })
                    receive?.({
                        event_type: 'delta',
                        data: { content: 'Checking the crash context.' },
                    })
                    assert.equal(events.at(-1)?.data.content, 'Checking the crash context.')
                    return {
                        role: 'assistant',
                        content: 'Checking the crash context.',
                        opaque: { reasoning: 'private' },
                        tool_calls: [
                            {
                                id: 'read',
                                name: 'read_log_file',
                                args: { file_id: 'log-1', start_line: 20 },
                            },
                        ],
                    }
                }
                receive?.({ event_type: 'delta', data: { content: 'Final diagnosis.' } })
                return { role: 'assistant', content: 'Final diagnosis.' }
            },
            tool: async () => ({ name: 'latest.log', lines: [{ line: 20, text: 'evidence' }] }),
        },
        new AbortController().signal,
        () => {},
        (event) => events.push(event),
    )
    assert.equal(output, 'Final diagnosis.')
    assert.deepEqual(
        events.map((event) => event.event_type),
        [
            'model_start',
            'thinking',
            'delta',
            'model_end',
            'tool',
            'tool_result',
            'model_start',
            'delta',
            'model_end',
        ],
    )
    assert.equal(events.find((event) => event.event_type === 'tool_result')?.data.lines, 1)
    assert.doesNotMatch(JSON.stringify(events), /private|evidence/)
})

test('repeated provider tool-call IDs still execute every investigation round', async () => {
    let turns = 0
    const reads: string[] = []
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async () => {
                turns++
                if (turns <= 2)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'call_0',
                                name: 'read_log_file',
                                args: { file_id: `log-${turns}` },
                            },
                        ],
                    }
                return { role: 'assistant', content: 'Final diagnosis after two reads.' }
            },
            tool: async (request) => {
                reads.push(request.file_id!)
                return 'evidence'
            },
        },
        new AbortController().signal,
        () => {},
    )
    assert.deepEqual(reads, ['log-1', 'log-2'])
    assert.equal(turns, 3)
    assert.match(output, /Final diagnosis/)
})

test('an unsupported extraction tool is returned to the model as an error instead of ending analysis', async () => {
    let turns = 0
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [{ id: 'extract-1', name: 'extract-diagnosis', args: {} }],
                    }
                const error = turn.messages.find((message) => message.role === 'tool')
                assert.match(error?.content ?? '', /not available/)
                return { role: 'assistant', content: 'Final diagnosis.' }
            },
            tool: async () => {
                throw new Error('Unsupported tools must not reach Rust')
            },
        },
        new AbortController().signal,
        () => {},
    )
    assert.equal(output, 'Final diagnosis.')
    assert.equal(turns, 2)
})

test('repeated unavailable tools stop with the tool name and do not execute', async () => {
    let turns = 0
    await assert.rejects(
        runLocalAgent(
            session,
            {
                modelTurn: async () => ({
                    role: 'assistant',
                    content: '',
                    tool_calls: [{ id: `call-${++turns}`, name: 'extract-diagnosis', args: {} }],
                }),
                tool: async () => {
                    throw new Error('Must not execute')
                },
            },
            new AbortController().signal,
            () => {},
        ),
        /repeatedly requested unavailable tools: extract-diagnosis/,
    )
    assert.equal(turns, 3)
})

test('the full 16-turn budget supports multiple investigation rounds before finalizing', async () => {
    let turns = 0
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async () =>
                ++turns < 16
                    ? {
                          role: 'assistant',
                          content: '',
                          tool_calls: [
                              {
                                  id: `call-${turns}`,
                                  name: 'read_log_file',
                                  args: { file_id: 'log-1', start_line: turns * 20 },
                              },
                          ],
                      }
                    : { role: 'assistant', content: 'Diagnosis after fifteen tool rounds.' },
            tool: async () => 'evidence',
        },
        new AbortController().signal,
        () => {},
    )
    assert.equal(turns, 16)
    assert.match(output, /Diagnosis/)
})

test('Deep Agents uses only diagnostic tools and preserves tool IDs and opaque protocol state', async () => {
    const turns: ModelTurn[] = []
    const used: string[] = []
    const gateway: AgentGateway = {
        async modelTurn(turn) {
            turns.push(turn)
            assert.deepEqual(
                turn.tools.map((tool) => (tool.function as { name: string }).name).sort(),
                ['grep_log_file', 'list_log_files', 'read_log_file'],
            )
            if (turns.length === 1)
                return {
                    role: 'assistant',
                    content: '',
                    tool_calls: [
                        {
                            id: 'call-123',
                            name: 'read_log_file',
                            args: { file_id: 'log-1', start_line: 200 },
                        },
                    ],
                    opaque: { signature: 'provider-required' },
                }
            assert.equal(
                turn.messages.find((message) => message.role === 'tool')?.tool_call_id,
                'call-123',
            )
            assert.deepEqual(
                turn.messages.find((message) => message.role === 'assistant')?.opaque,
                { signature: 'provider-required' },
            )
            return {
                role: 'assistant',
                content: 'The log identifies an incompatible Java version.',
            }
        },
        async tool(request) {
            assert.equal(request.file_id, 'log-1')
            return { line: 200, text: 'UnsupportedClassVersionError' }
        },
    }
    const output = await runLocalAgent(session, gateway, new AbortController().signal, (tool) =>
        used.push(tool),
    )
    assert.equal(turns.length, 2)
    assert.deepEqual(used, ['read_log_file'])
    assert.match(output, /Java/)
})

test('an aborted run does not call the model', async () => {
    const controller = new AbortController()
    controller.abort()
    const gateway: AgentGateway = {
        modelTurn: async () => {
            throw new Error('must not run')
        },
        tool: async () => {
            throw new Error('must not run')
        },
    }
    await assert.rejects(runLocalAgent(session, gateway, controller.signal, () => {}))
})

test('accepts standard content blocks without displaying reasoning blocks', () => {
    const message = new AIMessage({
        contentBlocks: [
            { type: 'reasoning', reasoning: 'Internal reasoning' },
            { type: 'text', text: 'Java is incompatible.\n' },
            { type: 'text', text: 'Select a supported Java version.' },
        ],
    })
    assert.equal(diagnosticText(message), 'Java is incompatible.\nSelect a supported Java version.')
})

test('the complete agent loop accepts a final response normalized into content blocks', async (context) => {
    const generate = LogAgentModel.prototype._generate
    context.mock.method(
        LogAgentModel.prototype,
        '_generate',
        async function (this: LogAgentModel, ...args: Parameters<typeof generate>) {
            const result = await generate.apply(this, args)
            for (const generation of result.generations) {
                generation.message = new AIMessage({
                    ...generation.message,
                    contentBlocks: generation.message.contentBlocks,
                })
            }
            return result
        },
    )
    const output = await runLocalAgent(
        session,
        {
            modelTurn: async () => ({
                role: 'assistant',
                content: '诊断结果：Java 版本不兼容。',
                tool_calls: [],
            }),
            tool: async () => {
                throw new Error('No tool needed for this response')
            },
        },
        new AbortController().signal,
        () => {},
    )
    assert.equal(output, '诊断结果：Java 版本不兼容。')
})

test('does not treat tool output or unfinished tool calls as a final diagnosis', () => {
    assert.throws(
        () => diagnosticText(new ToolMessage({ content: 'log evidence', tool_call_id: 'call-1' })),
        /without a final assistant reply/,
    )
    assert.throws(
        () =>
            diagnosticText(
                new AIMessage({
                    content: 'Checking logs',
                    tool_calls: [{ id: 'call-1', name: 'list_log_files', args: {} }],
                }),
            ),
        /unresolved tool calls/,
    )
    assert.throws(
        () =>
            diagnosticText(
                new AIMessage({
                    contentBlocks: [{ type: 'reasoning', reasoning: 'Internal reasoning' }],
                }),
            ),
        /content: reasoning/,
    )
})
