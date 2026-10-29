import assert from 'node:assert/strict'
import { test } from 'node:test'

import { effectScope } from 'vue'

import { runLocalAgent } from '../logagent/agent.ts'
import type { AnalysisEvent, PreparedSession, RemoteMcpGateway } from '../logagent/types.ts'
import { useLogAgent } from './useLogAgent.ts'

function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => {
        resolve = done
    })
    return { promise, resolve }
}

test('live model text updates before completion, tool notes stay in progress, and late deltas are ignored after cancel', async () => {
    const finish = deferred<string>()
    let progress!: (event: import('../logagent/types.ts').LocalProgress) => void
    const started = deferred<void>()
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, id) => ({
                run_id: id,
                provider_id: 'provider',
                model_id: 'model',
                locale: '',
                context: '',
            }),
            cancel: async () => {},
            cloud: async () => {
                throw new Error('Must not use cloud')
            },
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
            local: async (_session, _gateway, _signal, _tool, onProgress) => {
                progress = onProgress!
                started.resolve()
                return finish.promise
            },
        }),
    )!
    const pending = agent.start('instance', 'custom', null, () => {})
    await started.promise
    progress({ event_type: 'model_start', data: { turn: 1 } })
    progress({ event_type: 'delta', data: { content: 'Checking Java.' } })
    assert.equal(agent.output.value, 'Checking Java.')
    progress({ event_type: 'model_end', data: { content: 'Checking Java.', has_tools: true } })
    assert.equal(agent.output.value, '')
    assert.equal(agent.activity.value[1].text, 'Checking Java.')
    progress({ event_type: 'model_start', data: { turn: 2 } })
    progress({ event_type: 'delta', data: { content: 'The cause is' } })
    agent.cancel()
    progress({ event_type: 'delta', data: { content: ' stale text' } })
    assert.equal(agent.output.value, 'The cause is')
    assert.equal(agent.activity.value.at(-1)?.state, 'error')
    finish.resolve('old result')
    await pending
    assert.equal(agent.status.value, 'cancelled')
    assert.equal(agent.output.value, 'The cause is')
    scope.stop()
})

test('passes a user question to the local log agent', async () => {
    let receivedQuestion: string | undefined
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, id) => ({
                run_id: id,
                provider_id: 'provider',
                model_id: 'model',
                locale: 'zh-CN',
                context: 'latest.log is available.',
            }),
            cancel: async () => {},
            cloud: async () => {
                throw new Error('Must not use cloud')
            },
            local: async (_session, _gateway, _signal, _tool, _progress, question) => {
                receivedQuestion = question
                return 'The requested log analysis.'
            },
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!

    await agent.start('instance', 'custom', null, () => {}, 'Find the first error.')

    assert.equal(receivedQuestion, 'Find the first error.')
    assert.equal(agent.output.value, 'The requested log analysis.')
    assert.equal(agent.status.value, 'ready')
    scope.stop()
})

test('prepares an assistant session without an instance and forwards the selected model', async () => {
    let prepared: unknown[] = []
    let receivedMode: string | undefined
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (...args) => {
                prepared = args
                return {
                    run_id: args[1],
                    provider_id: args[3] ?? 'provider',
                    model_id: args[4] ?? 'model',
                    locale: 'zh-CN',
                    context: 'Read-only launcher context.',
                }
            },
            cancel: async () => {},
            cloud: async () => {
                throw new Error('Must not use cloud')
            },
            local: async (_session, _gateway, _signal, _tool, _progress, _question, _mcp, mode) => {
                receivedMode = mode
                return 'Ready to help.'
            },
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!

    await agent.start('', 'assistant', null, () => {}, 'How do I change memory?', undefined, {
        providerId: 'openai',
        modelId: 'gpt-4o-mini',
    })

    assert.equal(prepared[0], '')
    assert.equal(prepared[2], 'assistant')
    assert.equal(prepared[3], 'openai')
    assert.equal(prepared[4], 'gpt-4o-mini')
    assert.equal(receivedMode, 'assistant')
    assert.equal(agent.output.value, 'Ready to help.')
    scope.stop()
})

test('keeps structured Tauri provider errors readable for the assistant alert', async () => {
    const providerError = {
        field_name: 'Theseus',
        message: 'AI_AUTHENTICATION_FAILED: DeepSeek returned HTTP 401 Unauthorized',
    }
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async () => {
                throw providerError
            },
            cancel: async () => {},
            cloud: async () => {
                throw new Error('Must not use cloud')
            },
            local: async () => 'unused',
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!

    await agent.start('', 'assistant', null, () => {}, 'Why did authentication fail?')

    assert.equal(agent.status.value, 'error')
    assert.equal(agent.error.value, providerError.message)
    assert.doesNotMatch(agent.error.value, /\[object Object\]/)
    scope.stop()
})

test('a configured MCP tool reaches the real agent through the composable and asks for approval', async () => {
    let modelTurns = 0
    let approvals = 0
    let calls = 0
    const remote: RemoteMcpGateway = {
        endpoint: 'https://mcp.example.test/mcp',
        tools: [{ name: 'server_status', inputSchema: { type: 'object' } }],
        approveTool: async (tool, args) => {
            approvals++
            assert.equal(tool.name, 'server_status')
            assert.deepEqual(args, { address: 'play.example.test' })
            return true
        },
        callTool: async (name, args, signal) => {
            calls++
            assert.equal(approvals, 1)
            assert.equal(name, 'server_status')
            assert.deepEqual(args, { address: 'play.example.test' })
            assert.equal(signal.aborted, false)
            return { online: true }
        },
    }
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, runId) => ({
                run_id: runId,
                provider_id: 'provider',
                model_id: 'model',
                locale: 'en-US',
                context: 'A server status check was requested.',
            }),
            cancel: async () => {},
            cloud: async () => {
                throw new Error('Must not use cloud')
            },
            local: runLocalAgent,
            gateway: () => ({
                modelTurn: async (turn) => {
                    if (++modelTurns === 1) {
                        assert.ok(
                            turn.tools.some(
                                (entry) =>
                                    (entry.function as { name: string }).name ===
                                    'mcp_0_server_status',
                            ),
                        )
                        return {
                            role: 'assistant',
                            content: '',
                            tool_calls: [
                                {
                                    id: 'server-status-1',
                                    name: 'mcp_0_server_status',
                                    args: { address: 'play.example.test' },
                                },
                            ],
                        }
                    }
                    assert.match(
                        turn.messages.find((message) => message.role === 'tool')?.content ?? '',
                        /online/,
                    )
                    return { role: 'assistant', content: 'The server is online.' }
                },
                tool: async () => {
                    throw new Error('No local log tool was requested')
                },
            }),
        }),
    )!
    try {
        await agent.start('instance', 'custom', null, () => {}, 'Is the server online?', remote)
        assert.equal(agent.status.value, 'ready')
        assert.equal(agent.output.value, 'The server is online.')
        assert.equal(approvals, 1)
        assert.equal(calls, 1)
        assert.equal(modelTurns, 2)
    } finally {
        scope.stop()
    }
})

test('cancelled preparation cannot start cloud work or overwrite a replacement run', async () => {
    const old = deferred<PreparedSession>()
    let prepares = 0
    let clouds = 0
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, id) =>
                ++prepares === 1
                    ? old.promise
                    : {
                          run_id: id,
                          provider_id: 'provider',
                          model_id: 'model',
                          locale: 'zh-CN',
                          context: '',
                      },
            cancel: async () => {},
            cloud: async () => {
                clouds++
                return { content: 'cloud', ticket: null }
            },
            local: async () => 'new local result',
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!
    const first = agent.start('same-instance', 'logshare', null, () => {})
    agent.cancel()
    await agent.start('same-instance', 'custom', null, () => {})
    old.resolve({ run_id: 'old', provider_id: '', model_id: '', locale: '', context: '' })
    await first
    assert.equal(clouds, 0)
    assert.equal(agent.output.value, 'new local result')
    assert.equal(agent.status.value, 'ready')
    scope.stop()
})

test('cloud events are run-specific, errors can retry, local has no cloud fallback', async () => {
    let receive: ((event: AnalysisEvent) => void) | undefined
    const listening = deferred<void>()
    let runId = ''
    let cloudCalls = 0
    const cloud = deferred<{ content: string; ticket: null }>()
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, id) => {
                runId = id
                return {
                    run_id: id,
                    provider_id: 'provider',
                    model_id: 'model',
                    locale: '',
                    context: '',
                }
            },
            cancel: async () => {},
            cloud: async (_run, _ticket, listener) => {
                cloudCalls++
                receive = listener
                listening.resolve()
                return cloud.promise
            },
            local: async () => {
                throw new Error('model has no tool support')
            },
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!
    await agent.start('instance', 'custom', null, () => {})
    assert.equal(agent.status.value, 'error')
    assert.equal(cloudCalls, 0)
    const pending = agent.start('instance', 'logshare', null, () => {})
    await listening.promise
    receive!({ run_id: runId, event_type: 'queued', data: { position: 2 } })
    assert.equal(agent.event.value?.event_type, 'queued')
    receive!({ run_id: runId, event_type: 'thinking', data: { content: 'private' } })
    assert.equal(agent.event.value?.event_type, 'thinking')
    assert.deepEqual(agent.event.value?.data, {})
    receive!({ run_id: runId, event_type: 'queued', data: { position: 1 } })
    receive!({ run_id: 'old-run', event_type: 'delta', data: { content: 'wrong' } })
    receive!({ run_id: runId, event_type: 'delta', data: { content: 'right' } })
    assert.equal(agent.event.value, null)
    assert.equal(agent.output.value, 'right')
    cloud.resolve({ content: 'complete', ticket: null })
    await pending
    assert.equal(agent.status.value, 'ready')
    assert.equal(agent.output.value, 'complete')
    scope.stop()
})

test('reuses an in-flight summary upload and cancellation prevents the cloud request', async () => {
    const upload = deferred<import('../logagent/types.ts').UploadTicket | null>()
    let clouds = 0
    const scope = effectScope()
    const agent = scope.run(() =>
        useLogAgent({
            prepare: async (_instance, id) => ({
                run_id: id,
                provider_id: '',
                model_id: '',
                locale: '',
                context: '',
            }),
            cancel: async () => {},
            cloud: async (_id, ticket) => {
                clouds++
                assert.equal(ticket?.id, 'existing-upload')
                return { content: 'done', ticket }
            },
            local: async () => '',
            gateway: () => ({
                modelTurn: async () => {
                    throw new Error('unused')
                },
                tool: async () => null,
            }),
        }),
    )!
    const first = agent.start('instance', 'logshare', upload.promise, () => {})
    await Promise.resolve()
    assert.equal(agent.busy.value, true)
    agent.cancel()
    upload.resolve({ id: 'existing-upload', url: '', raw: '', token: '' })
    await first
    assert.equal(clouds, 0)
    await agent.start('instance', 'logshare', upload.promise, () => {})
    assert.equal(clouds, 1)
    scope.stop()
})
