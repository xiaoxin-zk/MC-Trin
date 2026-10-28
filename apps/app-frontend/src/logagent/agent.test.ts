import assert from 'node:assert/strict'
import { test } from 'node:test'

import { AIMessage, ToolMessage } from '@langchain/core/messages'

import { diagnosticText, runLocalAgent } from './agent.ts'
import { LogAgentModel, readableMessageContent } from './model.ts'
import type { AgentGateway, LocalProgress, ModelTurn } from './types.ts'

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

test('renders structured text blocks without coercing objects to [object Object]', () => {
    assert.equal(
        readableMessageContent([
            { type: 'text', text: { value: 'Java is incompatible.' } },
            { type: 'reasoning', reasoning: 'private' },
        ]),
        '{"value":"Java is incompatible."}',
    )
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

// The assistant exists to read and change launcher state on the user's behalf,
// so the write path (create / rename / delete / settings / content) is covered
// here rather than only the read-only diagnostics path.

test('write tools refuse to run without an approval handler', async () => {
    let turns = 0
    let toolCalls = 0
    let refusal = ''
    const output = await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'delete-1',
                                name: 'delete_instance',
                                args: { instance_id: 'inst-1' },
                            },
                        ],
                    }
                refusal = turn.messages.find((message) => message.role === 'tool')?.content ?? ''
                return { role: 'assistant', content: 'I need your approval before deleting it.' }
            },
            tool: async () => {
                toolCalls += 1
                return {}
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
    )

    assert.equal(toolCalls, 0, 'the launcher must not be touched without an approval handler')
    assert.match(refusal, /require an explicit user approval/)
    assert.equal(output, 'I need your approval before deleting it.')
})

test('a declined write keeps the instance untouched and tells the model', async () => {
    let turns = 0
    let toolCalls = 0
    let refusal = ''
    await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'delete-2',
                                name: 'delete_instance',
                                args: { instance_id: 'inst-2' },
                            },
                        ],
                    }
                refusal = turn.messages.find((message) => message.role === 'tool')?.content ?? ''
                return { role: 'assistant', content: 'Deletion cancelled.' }
            },
            tool: async () => {
                toolCalls += 1
                return {}
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async () => false,
    )

    assert.equal(toolCalls, 0, 'a declined write must not reach the launcher')
    assert.match(refusal, /did not approve/)
})

test('an approved write forwards the exact request to the launcher', async () => {
    let turns = 0
    const forwarded: Record<string, unknown>[] = []
    const output = await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'create-1',
                                name: 'create_instance',
                                args: {
                                    instance_name: '我的整合包',
                                    game_version: '1.21.1',
                                    loader: 'fabric',
                                },
                            },
                        ],
                    }
                assert.match(
                    turn.messages.find((message) => message.role === 'tool')?.content ?? '',
                    /new-instance/,
                )
                return { role: 'assistant', content: 'Created the instance.' }
            },
            tool: async (request) => {
                forwarded.push(request)
                return { instance_id: 'new-instance' }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async () => true,
    )

    assert.deepEqual(forwarded, [
        {
            name: 'create_instance',
            instance_name: '我的整合包',
            game_version: '1.21.1',
            loader: 'fabric',
        },
    ])
    assert.equal(output, 'Created the instance.')
})

test('write approval shows the tool name, its description and the write permission', async () => {
    let turns = 0
    const approvals: { name: string; description: string; permission: string }[] = []
    let approvalArgs: Record<string, unknown> = {}
    await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async () => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'settings-2',
                                name: 'update_launcher_settings',
                                args: { patch: { maximum_megabytes: 8192 } },
                            },
                        ],
                    }
                return { role: 'assistant', content: 'Memory raised to 8192 MB.' }
            },
            tool: async () => ({ ok: true }),
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async (approval, args) => {
            approvals.push(approval)
            approvalArgs = args
            return true
        },
    )

    assert.equal(approvals.length, 1)
    assert.equal(approvals[0].name, 'update_launcher_settings')
    assert.equal(approvals[0].permission, 'write')
    assert.ok(
        approvals[0].description.length > 0,
        'the approval prompt needs a human readable description',
    )
    assert.deepEqual(approvalArgs, { patch: { maximum_megabytes: 8192 } })
})

test('read-only tools run without an approval handler', async () => {
    let turns = 0
    const forwarded: string[] = []
    await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async () => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [{ id: 'overview-1', name: 'get_instance_overview', args: {} }],
                    }
                return { role: 'assistant', content: 'You have one instance.' }
            },
            tool: async (request) => {
                forwarded.push(request.name)
                return { instances: [{ id: 'inst-1', name: 'Vanilla' }] }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
    )

    assert.deepEqual(forwarded, ['get_instance_overview'])
})

test('invalid write arguments never reach the launcher', async () => {
    let turns = 0
    let toolCalls = 0
    try {
        await runLocalAgent(
            { ...session, context: 'Launcher assistant session.' },
            {
                modelTurn: async () => {
                    if (++turns === 1)
                        return {
                            role: 'assistant',
                            content: '',
                            tool_calls: [
                                {
                                    id: 'create-2',
                                    name: 'create_instance',
                                    args: { instance_name: '', game_version: '1.21.1' },
                                },
                            ],
                        }
                    return { role: 'assistant', content: 'That name is not usable.' }
                },
                tool: async () => {
                    toolCalls += 1
                    return {}
                },
            },
            new AbortController().signal,
            () => {},
            undefined,
            undefined,
            undefined,
            'assistant',
            '',
            async () => true,
        )
    } catch {
        // A schema failure may surface as an error or as a tool error message;
        // either way the launcher must not run the write.
    }

    assert.equal(toolCalls, 0, 'invalid arguments must be rejected before the write')
})

test('cancelling during a tool call stops the run without reporting a finished tool', async () => {
    const controller = new AbortController()
    const progresses: LocalProgress[] = []
    let turns = 0
    await assert.rejects(
        runLocalAgent(
            { ...session, context: 'Launcher assistant session.' },
            {
                modelTurn: async () => {
                    turns += 1
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'install-cancelled',
                                name: 'install_java_runtime',
                                args: { java_version: 21, vendor: 'temurin' },
                            },
                        ],
                    }
                },
                tool: async () => {
                    controller.abort()
                    return { job_id: 'java-job-1' }
                },
            },
            controller.signal,
            () => {},
            (event) => progresses.push(event),
            undefined,
            undefined,
            'assistant',
            '',
            async () => true,
        ),
        (error: unknown) => {
            const candidate = error as { name?: string; message?: string }
            return candidate?.name === 'AbortError' || /abort/i.test(candidate?.message ?? '')
        },
    )

    assert.equal(turns, 1, 'an aborted tool call must not start another model turn')
    assert.equal(
        progresses.filter((event) => event.event_type === 'tool').length,
        1,
        'the cancelled tool should still be announced to the user',
    )
    assert.deepEqual(
        progresses.filter((event) => event.event_type === 'tool_result'),
        [],
        'a cancelled tool must not report a finished result',
    )
})

test('a failing approved write never claims success and keeps the failure visible', async () => {
    let turns = 0
    let toolFailure = ''
    const progresses: LocalProgress[] = []
    let caught: unknown
    let output = ''
    try {
        output = await runLocalAgent(
            { ...session, context: 'Launcher assistant session.' },
            {
                modelTurn: async (turn) => {
                    if (++turns === 1)
                        return {
                            role: 'assistant',
                            content: '',
                            tool_calls: [
                                {
                                    id: 'rename-fails',
                                    name: 'rename_instance',
                                    args: { instance_id: 'inst-1', new_name: 'Renamed' },
                                },
                            ],
                        }
                    toolFailure = turn.messages
                        .filter((message) => message.role === 'tool')
                        .map((message) => message.content)
                        .join('\n')
                    return {
                        role: 'assistant',
                        content: 'The rename failed; nothing changed.',
                    }
                },
                tool: async () => {
                    throw new Error('instance is locked by a running game')
                },
            },
            new AbortController().signal,
            () => {},
            (event) => progresses.push(event),
            undefined,
            undefined,
            'assistant',
            '',
            async () => true,
        )
    } catch (error) {
        caught = error
    }

    // The graph may hand the failure back to the model or abort the run; either
    // way the launcher must never report that the write finished.
    if (caught) {
        assert.match(String((caught as Error)?.message ?? caught), /instance is locked/)
    } else {
        assert.match(toolFailure, /instance is locked/)
        assert.match(output, /nothing changed/)
    }
    assert.ok(
        progresses.some(
            (event) => event.event_type === 'tool_result' && event.data.failed === true,
        ),
        'the failed write must be reported as a failure',
    )
})

test('a Java install job can be cancelled by the id it returned', async () => {
    let turns = 0
    const forwarded: string[] = []
    const jobIds: unknown[] = []
    const approvals: string[] = []
    const output = await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async () => {
                turns += 1
                if (turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'install-1',
                                name: 'install_java_runtime',
                                args: { java_version: 21, vendor: 'temurin' },
                            },
                        ],
                    }
                if (turns === 2)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'cancel-1',
                                name: 'cancel_install_job',
                                args: { job_id: 'java-job-9' },
                            },
                        ],
                    }
                return {
                    role: 'assistant',
                    content: 'The Java download was cancelled.',
                }
            },
            tool: async (request) => {
                forwarded.push(request.name)
                jobIds.push(request.job_id)
                if (request.name === 'install_java_runtime')
                    return { job_id: 'java-job-9', status: 'queued' }
                return { job_id: 'java-job-9', status: 'cancelled' }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async (approval) => {
            approvals.push(approval.name)
            return true
        },
    )

    assert.deepEqual(forwarded, ['install_java_runtime', 'cancel_install_job'])
    assert.equal(jobIds[1], 'java-job-9', 'the exact job id has to reach the launcher')
    assert.deepEqual(approvals, ['install_java_runtime', 'cancel_install_job'])
    assert.match(output, /cancelled/)
})

test('a prepared resource plan can be queued and its job status followed', async () => {
    let turns = 0
    const forwarded: string[] = []
    const statuses: string[] = []
    const output = await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async () => {
                turns += 1
                if (turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'prepare-1',
                                name: 'prepare_resource_install',
                                args: {
                                    instance_id: 'inst-1',
                                    source: 'modrinth',
                                    project_id: 'sodium',
                                    resource_type: 'mod',
                                },
                            },
                        ],
                    }
                if (turns === 2)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'queue-1',
                                name: 'queue_resource_install',
                                args: { plan_id: 'plan-1', plan_revision: 3 },
                            },
                        ],
                    }
                if (turns === 3)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'status-1',
                                name: 'get_install_job',
                                args: { job_id: 'job-1' },
                            },
                        ],
                    }
                return { role: 'assistant', content: 'Sodium is installed.' }
            },
            tool: async (request) => {
                forwarded.push(request.name)
                if (request.name === 'prepare_resource_install')
                    return { plan_id: 'plan-1', revision: 3, entries: [{ project_id: 'sodium' }] }
                if (request.name === 'queue_resource_install')
                    return { job_id: 'job-1', status: 'queued' }
                const status = 'installed'
                statuses.push(status)
                return { job_id: 'job-1', status, progress: 1 }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async () => true,
    )

    assert.deepEqual(forwarded, [
        'prepare_resource_install',
        'queue_resource_install',
        'get_install_job',
    ])
    assert.deepEqual(statuses, ['installed'], 'the final job status has to be observed')
    assert.match(output, /Sodium is installed/)
})

test('queueing a plan that was not prepared in this session never reaches approval', async () => {
    let turns = 0
    let planError = ''
    const forwarded: string[] = []
    const approvals: string[] = []
    await runLocalAgent(
        { ...session, context: 'Launcher assistant session.' },
        {
            modelTurn: async (turn) => {
                if (++turns === 1)
                    return {
                        role: 'assistant',
                        content: '',
                        tool_calls: [
                            {
                                id: 'queue-unknown',
                                name: 'queue_resource_install',
                                args: { plan_id: 'unknown-plan', plan_revision: 1 },
                            },
                        ],
                    }
                planError = turn.messages
                    .filter((message) => message.role === 'tool')
                    .map((message) => message.content)
                    .join('\n')
                return { role: 'assistant', content: 'That plan has to be prepared first.' }
            },
            tool: async (request) => {
                forwarded.push(request.name)
                return { job_id: 'job-1' }
            },
        },
        new AbortController().signal,
        () => {},
        undefined,
        undefined,
        undefined,
        'assistant',
        '',
        async (approval) => {
            approvals.push(approval.name)
            return true
        },
    )

    assert.deepEqual(forwarded, [], 'an unprepared plan must never reach the launcher')
    assert.deepEqual(approvals, [], 'the user must not be asked to approve an unprepared plan')
    assert.match(planError, /Prepare this resource plan/)
})
