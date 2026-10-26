import { AIMessage, type BaseMessage, ToolMessage } from '@langchain/core/messages'
import { tool } from '@langchain/core/tools'
import { createDeepAgent, registerHarnessProfile } from 'deepagents/browser'
import { createMiddleware } from 'langchain'
import { z } from 'zod'

import { LogAgentModel } from './model.ts'
import { LAUNCHER_ASSISTANT_PROMPT, LOGAGENT_PROMPT } from './prompts.ts'
import { enabledAgentTools } from './tool-settings.ts'
import type {
    AgentGateway,
    AgentToolApproval,
    LocalProgress,
    PreparedSession,
    RemoteMcpGateway,
} from './types.ts'

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
    approveLocalTool?: (tool: AgentToolApproval, args: Record<string, unknown>) => Promise<boolean>,
): Promise<string> {
    const resourcePlans = new Map<string, Record<string, unknown>>()
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
        if (name === 'prepare_resource_install' && typeof info.plan_id === 'string') {
            resourcePlans.set(info.plan_id, info)
            while (resourcePlans.size > 8) {
                const oldest = resourcePlans.keys().next().value
                if (!oldest) break
                resourcePlans.delete(oldest)
            }
        }
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
    const invokeWriteTool = async (
        name: string,
        description: string,
        request: Record<string, unknown>,
        approvalRequest: Record<string, unknown> = request,
    ) => {
        signal.throwIfAborted()
        if (!approveLocalTool)
            return JSON.stringify({ error: 'Write tools require an explicit user approval.' })
        const approved = await approveLocalTool(
            { name, description, permission: 'write' },
            approvalRequest,
        )
        signal.throwIfAborted()
        if (!approved) return JSON.stringify({ error: 'The user did not approve this write.' })
        return invokeTool(name, request)
    }
    const enabledBuiltInTools = enabledAgentTools(mode)
    const builtInTools = (
        mode === 'assistant'
            ? [
                  tool(() => invokeTool('get_instance_overview', {}), {
                      name: 'get_instance_overview',
                      description:
                          'List installed Minecraft instances with stable IDs, names, game versions, loaders, and install status. Paths are never returned.',
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
                  tool(() => invokeTool('discover_java_installations', {}), {
                      name: 'discover_java_installations',
                      description:
                          'Find installed Java runtimes using the launcher discovery cache and safe system scan. Paths are never returned.',
                      schema: z.object({}),
                  }),
                  tool(() => invokeTool('list_java_vendors', {}), {
                      name: 'list_java_vendors',
                      description:
                          'List Java vendors currently available for this platform from the launcher feed.',
                      schema: z.object({}),
                  }),
                  tool(
                      (request) => invokeTool('inspect_instance_content', request),
                      {
                          name: 'inspect_instance_content',
                          description:
                              'Inspect an instance content snapshot for installed mods, resource packs, shaders, versions, missing content, updates, and compatibility warnings. Paths remain relative.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('check_content_updates', request),
                      {
                          name: 'check_content_updates',
                          description:
                              'Check whether installed content has compatible updates available, without applying them.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('search_resources', request),
                      {
                          name: 'search_resources',
                          description:
                              'Search the configured Modrinth, CurseForge, MCArchive, and Planet Minecraft resource sources for mods, resource packs, shaders, datapacks, worlds, or modpacks.',
                          schema: z.object({
                              query: z.string().min(1).max(200),
                              source: z
                                  .enum(['modrinth', 'curseforge', 'mcarchive', 'planet_minecraft'])
                                  .optional(),
                              resource_type: z
                                  .enum([
                                      'mod',
                                      'plugin',
                                      'data_pack',
                                      'resource_pack',
                                      'shader',
                                      'mod_pack',
                                      'world',
                                  ])
                                  .optional(),
                              game_version: z.string().max(32).optional(),
                              loader: z.string().max(32).optional(),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('get_resource_releases', request),
                      {
                          name: 'get_resource_releases',
                          description:
                              'List compatible releases for one resource project before preparing an install.',
                          schema: z.object({
                              source: z.enum(['modrinth', 'curseforge', 'mcarchive', 'planet_minecraft']),
                              project_id: z.string().min(1).max(160),
                              game_version: z.string().max(32).optional(),
                              loader: z.string().max(32).optional(),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('prepare_resource_install', request),
                      {
                          name: 'prepare_resource_install',
                          description:
                              'Create a dependency-resolved resource installation plan without downloading or changing game files. Use the plan summary to explain the proposed fix.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              source: z.enum(['modrinth', 'curseforge', 'mcarchive', 'planet_minecraft']),
                              project_id: z.string().min(1).max(160),
                              version_id: z.string().max(160).optional(),
                              resource_type: z.enum([
                                  'mod',
                                  'plugin',
                                  'data_pack',
                                  'resource_pack',
                                  'shader',
                                  'mod_pack',
                                  'world',
                              ]),
                              install_dependencies: z.boolean().optional(),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('prepare_resource_update', request),
                      {
                          name: 'prepare_resource_update',
                          description:
                              'Prepare a dependency-resolved update plan for one installed content entry without downloading or changing files.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              content_id: z.string().min(1).max(160),
                          }),
                      },
                  ),
                  tool(
                      (request) => invokeTool('get_install_job', request),
                      {
                          name: 'get_install_job',
                          description:
                              'Read the status, phase, progress, and error of a launcher download or installation job.',
                          schema: z.object({ job_id: z.string().min(1).max(80) }),
                      },
                  ),
                  tool(() => invokeTool('list_log_files', {}), {
                      name: 'list_log_files',
                      description: 'List the selected instance crash snapshot IDs, names, and original line ranges.',
                      schema: z.object({}),
                  }),
                  tool((request) => invokeTool('read_log_file', request), {
                      name: 'read_log_file',
                      description: 'Read a bounded range from the selected instance crash snapshot.',
                      schema: z.object({
                          file_id: z.string(),
                          start_line: z.number().int().positive().optional(),
                          end_line: z.number().int().positive().optional(),
                      }),
                  }),
                  tool((request) => invokeTool('grep_log_file', request), {
                      name: 'grep_log_file',
                      description: 'Search the selected instance crash snapshot using a literal query and surrounding context.',
                      schema: z.object({
                          file_id: z.string(),
                          query: z.string().min(1).max(256),
                          context: z.number().int().min(0).max(10).optional(),
                      }),
                  }),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'update_launcher_settings',
                              'Update a small allowlist of non-sensitive launcher preferences.',
                              request,
                          ),
                      {
                          name: 'update_launcher_settings',
                          description:
                              'Update non-sensitive launcher preferences such as language, theme, memory, download source, and concurrency. Credentials, paths, hooks, telemetry, and proxy settings are not accepted. Every call requires user approval.',
                          schema: z.object({
                              patch: z.record(z.string(), z.unknown()).min(1).max(8),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'create_instance',
                              'Create an empty Minecraft instance record for the user to install.',
                              request,
                          ),
                      {
                          name: 'create_instance',
                          description:
                              'Create an empty Minecraft instance record using a name, game version, and optional loader. It does not download files. Every call requires user approval.',
                          schema: z.object({
                              instance_name: z.string().min(1).max(100),
                              game_version: z.string().min(1).max(64),
                              loader: z
                                  .enum([
                                      'vanilla',
                                      'forge',
                                      'fabric',
                                      'quilt',
                                      'neoforge',
                                      'optifine',
                                      'cleanroom',
                                      'lite_loader',
                                      'legacy_fabric',
                                      'babric',
                                  ])
                                  .optional(),
                              loader_version: z.string().max(64).optional(),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'rename_instance',
                              'Rename one Minecraft instance without exposing its path.',
                              request,
                          ),
                      {
                          name: 'rename_instance',
                          description:
                              'Rename an existing Minecraft instance by ID. Every call requires user approval.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              instance_name: z.string().min(1).max(100),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'delete_instance',
                              'Delete one Minecraft instance and its managed files.',
                              request,
                          ),
                      {
                          name: 'delete_instance',
                          description:
                              'Delete an existing Minecraft instance by ID. This can remove managed files and every call requires user approval.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'install_java_runtime',
                              'Download and install a Java runtime through the launcher download engine.',
                              request,
                          ),
                      {
                          name: 'install_java_runtime',
                          description:
                              'Download and install Java 8, 17, 21, or 25 from a vendor returned by list_java_vendors. Every install requires approval and returns a cancellable job without exposing paths.',
                          schema: z.object({
                              java_version: z.number().int().min(8).max(25),
                              vendor: z.string().min(1).max(64),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'set_java_default_runtime',
                              'Set an installed Java runtime as the launcher default for its major version.',
                              request,
                          ),
                      {
                          name: 'set_java_default_runtime',
                          description:
                              'Select the first locally discovered Java runtime for a major version as its default. The path stays private and every change requires approval.',
                          schema: z.object({
                              java_version: z.number().int().min(8).max(25),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'cancel_install_job',
                              'Cancel a running launcher download or installation job.',
                              request,
                          ),
                      {
                          name: 'cancel_install_job',
                          description:
                              'Cancel a running Java or resource installation job after approval. The job may already have completed.',
                          schema: z.object({ job_id: z.string().min(1).max(80) }),
                      },
                  ),
                  tool(
                      (request) => {
                          const preview =
                              typeof request.plan_id === 'string'
                                  ? resourcePlans.get(request.plan_id)
                                  : undefined
                          if (!preview || preview.revision !== request.plan_revision)
                              return JSON.stringify({
                                  error: 'Prepare this resource plan in the current assistant session before queueing it.',
                              })
                          return invokeWriteTool(
                              'queue_resource_install',
                              'Queue a previously reviewed dependency-resolved resource plan for download and installation.',
                              request,
                              { ...request, plan_preview: preview },
                          )
                      },
                      {
                          name: 'queue_resource_install',
                          description:
                              'Start a resource installation or update plan returned by a prepare tool. The plan revision is checked before any files are changed. Every call requires approval.',
                          schema: z.object({
                              plan_id: z.string().min(1).max(160),
                              plan_revision: z.number().int().nonnegative(),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'disable_content',
                              'Disable one installed content entry so the user can test a reversible fix.',
                              request,
                          ),
                      {
                          name: 'disable_content',
                          description:
                              'Disable one identified mod, resource pack, shader, datapack, or other content entry. This is reversible and requires approval.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              content_id: z.string().min(1).max(160),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'enable_content',
                              'Re-enable one previously disabled content entry.',
                              request,
                          ),
                      {
                          name: 'enable_content',
                          description:
                              'Re-enable one identified content entry after a reversible test. Every call requires approval.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              content_id: z.string().min(1).max(160),
                          }),
                      },
                  ),
                  tool(
                      (request) =>
                          invokeWriteTool(
                              'remove_content',
                              'Remove one installed content entry from a managed instance.',
                              request,
                          ),
                      {
                          name: 'remove_content',
                          description:
                              'Remove one identified mod, resource pack, shader, or datapack. This can delete the managed content file and always requires approval.',
                          schema: z.object({
                              instance_id: z.string().min(1).max(128),
                              content_id: z.string().min(1).max(160),
                          }),
                      },
                  ),
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
                                    `This tool batch was not executed: ${names} is not available. Use only these currently enabled tools: ${Array.from(availableTools).join(', ') || 'none'}. Remote MCP tools still require approval.`,
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
