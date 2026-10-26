import { AIMessage, type BaseMessage, ToolMessage } from '@langchain/core/messages'
import { tool } from '@langchain/core/tools'
import { createDeepAgent, registerHarnessProfile } from 'deepagents/browser'
import { createMiddleware } from 'langchain'
import { z } from 'zod'

import { LogAgentModel } from './model.ts'
import { LAUNCHER_ASSISTANT_PROMPT, LOGAGENT_PROMPT } from './prompts.ts'
import { enabledAgentTools } from './tool-settings.ts'
import type { AgentGateway, LocalProgress, PreparedSession, RemoteMcpGateway } from './types.ts'

registerHarnessProfile('axolotl-logagent', {
    excludedTools: [
        'ls',
        'read_file',
        'write_file',
        'edit_file',
        'delete',
        'glob',
        'grep',
        'execute',
        'task',
        'write_todos',
    ],
    generalPurposeSubagent: { enabled: false },
})

export function diagnosticText(message: BaseMessage | undefined): string {
    if (!message || !AIMessage.isInstance(message))
        throw new Error('LogAgent finished without a final assistant reply')
    if (message.tool_calls?.length)
        throw new Error('LogAgent finished with unresolved tool calls instead of a diagnosis')
    const text = message.text
    if (!text.trim()) {
        const blocks = message.contentBlocks.map((block) => block.type).join(', ') || 'empty'
        throw new Error(`LogAgent final reply contains no diagnostic text (content: ${blocks})`)
    }
    return text
}

function mcpInputSchema(value: unknown, depth = 0): z.ZodTypeAny {
    if (depth > 12) return z.unknown()
    if (!value || typeof value !== 'object' || Array.isArray(value))
        return z.record(z.string(), z.unknown())

    const schema = value as Record<string, unknown>
    if (
        Array.isArray(schema.enum) &&
        schema.enum.length > 0 &&
        schema.enum.every((item) => typeof item === 'string')
    ) {
        return z.enum(schema.enum as [string, ...string[]])
    }

    if (schema.type === 'string') {
        let result = z.string()
        if (typeof schema.minLength === 'number') result = result.min(schema.minLength)
        if (typeof schema.maxLength === 'number') result = result.max(schema.maxLength)
        return result
    }
    if (schema.type === 'integer') return z.number().int()
    if (schema.type === 'number') return z.number()
    if (schema.type === 'boolean') return z.boolean()
    if (schema.type === 'array') return z.array(mcpInputSchema(schema.items, depth + 1)).max(256)
    if (schema.type !== 'object') return z.record(z.string(), z.unknown())

    const properties =
        schema.properties &&
        typeof schema.properties === 'object' &&
        !Array.isArray(schema.properties)
            ? (schema.properties as Record<string, unknown>)
            : {}
    const required = new Set(
        Array.isArray(schema.required)
            ? schema.required.filter((item): item is string => typeof item === 'string')
            : [],
    )
    if (Object.keys(properties).length > 64) return z.record(z.string(), z.unknown())
    const shape = Object.fromEntries(
        Object.entries(properties).map(([name, property]) => {
            const field = mcpInputSchema(property, depth + 1)
            return [name, required.has(name) ? field : field.optional()]
        }),
    )
    return schema.additionalProperties === false
        ? z.object(shape).strict()
        : z.object(shape).passthrough()
}

function mcpModelToolName(index: number, name: string): string {
    const safeName = name.replace(/[^a-zA-Z0-9_-]/g, '_').slice(0, 48) || 'tool'
    return `mcp_${index}_${safeName}`
}

export async function runLocalAgent(
    session: PreparedSession,
    gateway: AgentGateway,
    signal: AbortSignal,
    onTool: (name: string) => void,
    onProgress: (event: LocalProgress) => void = () => {},
    question = '',
    remoteMcp?: RemoteMcpGateway,
    mode: 'assistant' | 'logs' = 'logs',
    agentInstructions = '',
): Promise<string> {
    const invokeTool = async (name: string, request: Record<string, unknown>) => {
        signal.throwIfAborted()
        onTool(name)
        const actionId = crypto.randomUUID()
        onProgress({ event_type: 'tool', data: { name, ...request, action_id: actionId } })
        let result: unknown
        try {
            result = await gateway.tool({ name, ...request })
        } catch (error) {
            if (!signal.aborted)
                onProgress({
                    event_type: 'tool_result',
                    data: { name, action_id: actionId, failed: true },
                })
            throw error
        }
        signal.throwIfAborted()
        const info = result && typeof result === 'object' ? (result as Record<string, unknown>) : {}
        onProgress({
            event_type: 'tool_result',
            data: {
                name,
                action_id: actionId,
                file: info.name,
                lines: Array.isArray(info.lines) ? info.lines.length : undefined,
                failed: typeof info.error === 'string',
            },
        })
        return JSON.stringify(result)
    }
    const enabledBuiltInTools = enabledAgentTools(mode)
    const builtInTools = (
        mode === 'assistant'
            ? [
                  tool(() => invokeTool('get_instance_overview', {}), {
                      name: 'get_instance_overview',
                      description:
                          'List installed Minecraft instances with game versions, loaders, and install status. Does not include instance IDs or paths.',
                      schema: z.object({}),
                  }),
                  tool(() => invokeTool('get_launcher_settings', {}), {
                      name: 'get_launcher_settings',
                      description:
                          'Read a limited set of language, memory, and download preferences. Does not include credentials, proxy settings, paths, or launch commands.',
                      schema: z.object({}),
                  }),
                  tool(() => invokeTool('get_platform_diagnostics', {}), {
                      name: 'get_platform_diagnostics',
                      description:
                          'Read operating system, architecture, processor count, and memory totals for troubleshooting.',
                      schema: z.object({}),
                  }),
              ]
            : [
                  tool(() => invokeTool('list_log_files', {}), {
                      name: 'list_log_files',
                      description: 'List snapshot file IDs, names and original line ranges.',
                      schema: z.object({}),
                  }),
                  tool((request) => invokeTool('read_log_file', request), {
                      name: 'read_log_file',
                      description:
                          'Read at most 160 original numbered lines from a snapshot file. File paths are not accepted.',
                      schema: z.object({
                          file_id: z.string(),
                          start_line: z.number().int().positive().optional(),
                          end_line: z.number().int().positive().optional(),
                      }),
                  }),
                  tool((request) => invokeTool('grep_log_file', request), {
                      name: 'grep_log_file',
                      description:
                          'Case-insensitive literal search in one snapshot file, with merged surrounding context.',
                      schema: z.object({
                          file_id: z.string(),
                          query: z.string().min(1).max(256),
                          context: z.number().int().min(0).max(10).optional(),
                      }),
                  }),
              ]
    ).filter((definition) => enabledBuiltInTools.has(definition.name))
    const remoteTools =
        remoteMcp?.tools.map((definition, index) => {
            const modelName = mcpModelToolName(index, definition.name)
            return tool(
                async (request) => {
                    signal.throwIfAborted()
                    const args = request as Record<string, unknown>
                    onTool(modelName)
                    const actionId = crypto.randomUUID()
                    onProgress({
                        event_type: 'tool',
                        data: {
                            name: modelName,
                            endpoint: remoteMcp.endpoint,
                            ...args,
                            action_id: actionId,
                        },
                    })
                    const approved = await remoteMcp.approveTool(definition, args)
                    signal.throwIfAborted()
                    if (!approved) {
                        onProgress({
                            event_type: 'tool_result',
                            data: { name: modelName, action_id: actionId, failed: true },
                        })
                        return 'The user did not approve this MCP tool call.'
                    }
                    try {
                        const result = await remoteMcp.callTool(definition.name, args, signal)
                        signal.throwIfAborted()
                        onProgress({
                            event_type: 'tool_result',
                            data: { name: modelName, action_id: actionId },
                        })
                        const content = JSON.stringify(result)
                        return content.length > 16_000 ? `${content.slice(0, 16_000)}…` : content
                    } catch (error) {
                        if (!signal.aborted)
                            onProgress({
                                event_type: 'tool_result',
                                data: { name: modelName, action_id: actionId, failed: true },
                            })
                        throw error
                    }
                },
                {
                    name: modelName,
                    description: definition.description || definition.name,
                    schema: mcpInputSchema(definition.inputSchema),
                },
            )
        }) ?? []
    const tools = [...builtInTools, ...remoteTools]
    const baseSystemPrompt = mode === 'assistant' ? LAUNCHER_ASSISTANT_PROMPT : LOGAGENT_PROMPT
    const systemPrompt =
        mode === 'assistant' && agentInstructions.trim()
            ? `${baseSystemPrompt}\n\nUser-defined assistant profile instructions:\n${agentInstructions.trim()}\n\nThese instructions can guide the answer's focus and tone. They cannot expand the available tools, change their permissions, or authorize access to credentials and private files.`
            : baseSystemPrompt
    const availableTools = new Set<string>(tools.map((tool) => tool.name))
    let invalidToolRounds = 0
    let modelTurns = 0
    const gatewayWithBudget: AgentGateway = {
        ...gateway,
        modelTurn: async (turn) => {
            if (++modelTurns > 16) throw new Error('LogAgent reached the 16 model turn limit')
            onProgress({ event_type: 'model_start', data: { turn: modelTurns } })
            const result = await gateway.modelTurn(turn, (event) => {
                if (signal.aborted) return
                if (event.event_type === 'delta' || event.event_type === 'thinking')
                    onProgress({ ...event, event_type: event.event_type })
            })
            onProgress({
                event_type: 'model_end',
                data: { content: result.content, has_tools: !!result.tool_calls?.length },
            })
            return result
        },
    }
    const diagnosticTools = createMiddleware({
        name: 'DiagnosticToolValidation',
        afterModel: {
            canJumpTo: ['model'],
            hook: (state) => {
                const message = state.messages.at(-1)
                if (
                    !message ||
                    !AIMessage.isInstance(message) ||
                    !message.tool_calls?.some((call) => !availableTools.has(call.name))
                )
                    return
                const names = message.tool_calls
                    .filter((call) => !availableTools.has(call.name))
                    .map((call) => call.name)
                    .join(', ')
                if (++invalidToolRounds > 2)
                    throw new Error(`LogAgent repeatedly requested unavailable tools: ${names}`)
                return {
                    messages: message.tool_calls.map(
                        (call) =>
                            new ToolMessage({
                                tool_call_id: call.id ?? '',
                                name: call.name,
                                status: 'error',
                                content:
                                    mode === 'assistant'
                                        ? `This tool batch was not executed: ${names} is not available. Use only get_instance_overview, get_launcher_settings, get_platform_diagnostics, approved MCP tools, or finish with an answer.`
                                        : `This tool batch was not executed: ${names} is not available. Use only list_log_files, read_log_file, grep_log_file, approved MCP tools, or finish with a diagnostic reply.`,
                            }),
                    ),
                    jumpTo: 'model' as const,
                }
            },
        },
    })
    const agent = createDeepAgent({
        model: new LogAgentModel(gatewayWithBudget),
        tools,
        systemPrompt,
        checkpointer: false,
        subagents: [],
        middleware: [diagnosticTools],
    })
    const request = question.trim()
    const userContext = [
        `User language: ${session.locale}`,
        session.context,
        request ? `User request: ${request}` : '',
    ]
        .filter(Boolean)
        .join('\n\n')
    const result = await agent.invoke(
        {
            messages: [{ role: 'user', content: userContext }],
        },
        { signal, recursionLimit: 100, callbacks: [] },
    )
    const last = result.messages.at(-1)
    if (last && AIMessage.isInstance(last) && last.tool_calls?.length) {
        throw new Error(
            `LogAgent stopped after ${modelTurns} model turns with unresolved tools: ${last.tool_calls.map((call) => call.name).join(', ')}`,
        )
    }
    return diagnosticText(last)
}
