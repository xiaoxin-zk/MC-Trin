export type AgentMode = 'assistant' | 'logs'
export type AgentToolPermission = 'read' | 'write'

/** How much the assistant may do without asking the user first. */
export type AssistantPermissionMode = 'default' | 'free' | 'full'

export type BuiltInToolDefinition = {
    id: string
    mode: AgentMode
    permission: AgentToolPermission
}

/**
 * The local tools are deliberately narrow. Read tools expose sanitized launcher
 * context; write tools are limited to launcher settings, Java installation,
 * resource jobs, and instance content lifecycle actions. Write tools require
 * an explicit approval before the call unless the permission mode relaxes it.
 */
export const DANGEROUS_AGENT_TOOLS: ReadonlySet<string> = new Set([
    'delete_instance',
    'remove_content',
    'update_launcher_settings',
    'install_java_runtime',
    'queue_resource_install',
])
export const BUILT_IN_AGENT_TOOLS: BuiltInToolDefinition[] = [
    { id: 'get_instance_overview', mode: 'assistant', permission: 'read' },
    { id: 'get_launcher_settings', mode: 'assistant', permission: 'read' },
    { id: 'get_platform_diagnostics', mode: 'assistant', permission: 'read' },
    { id: 'discover_java_installations', mode: 'assistant', permission: 'read' },
    { id: 'list_java_vendors', mode: 'assistant', permission: 'read' },
    { id: 'inspect_instance_content', mode: 'assistant', permission: 'read' },
    { id: 'check_content_updates', mode: 'assistant', permission: 'read' },
    { id: 'search_resources', mode: 'assistant', permission: 'read' },
    { id: 'get_resource_releases', mode: 'assistant', permission: 'read' },
    { id: 'prepare_resource_install', mode: 'assistant', permission: 'read' },
    { id: 'prepare_resource_update', mode: 'assistant', permission: 'read' },
    { id: 'get_install_job', mode: 'assistant', permission: 'read' },
    { id: 'cancel_install_job', mode: 'assistant', permission: 'write' },
    { id: 'update_launcher_settings', mode: 'assistant', permission: 'write' },
    { id: 'create_instance', mode: 'assistant', permission: 'write' },
    { id: 'rename_instance', mode: 'assistant', permission: 'write' },
    { id: 'delete_instance', mode: 'assistant', permission: 'write' },
    { id: 'install_java_runtime', mode: 'assistant', permission: 'write' },
    { id: 'set_java_default_runtime', mode: 'assistant', permission: 'write' },
    { id: 'queue_resource_install', mode: 'assistant', permission: 'write' },
    { id: 'disable_content', mode: 'assistant', permission: 'write' },
    { id: 'enable_content', mode: 'assistant', permission: 'write' },
    { id: 'remove_content', mode: 'assistant', permission: 'write' },
    { id: 'list_log_files', mode: 'logs', permission: 'read' },
    { id: 'read_log_file', mode: 'logs', permission: 'read' },
    { id: 'grep_log_file', mode: 'logs', permission: 'read' },
]

const STORAGE_KEY = 'mctrin.assistant.tool-settings.v1'

type StoredToolSettings = {
    disabled: string[]
}

function read(): StoredToolSettings {
    try {
        const raw = localStorage.getItem(STORAGE_KEY)
        if (!raw) return { disabled: [] }
        const parsed = JSON.parse(raw) as Partial<StoredToolSettings>
        const disabled = Array.isArray(parsed.disabled)
            ? parsed.disabled.filter(
                  (id): id is string =>
                      typeof id === 'string' && BUILT_IN_AGENT_TOOLS.some((tool) => tool.id === id),
              )
            : []
        return { disabled: [...new Set(disabled)] }
    } catch {
        return { disabled: [] }
    }
}

function write(settings: StoredToolSettings) {
    try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(settings))
    } catch {
        // A blocked storage area only means the default tool set is used next
        // time; the current session keeps working with the in-memory choice.
    }
}

export function disabledAgentTools(): Set<string> {
    return new Set(read().disabled)
}

export function agentToolEnabled(id: string): boolean {
    return !read().disabled.includes(id)
}

export function setAgentToolEnabled(id: string, enabled: boolean): void {
    if (!BUILT_IN_AGENT_TOOLS.some((tool) => tool.id === id)) return
    const settings = read()
    const next = new Set(settings.disabled)
    if (enabled) next.delete(id)
    else next.add(id)
    write({ disabled: [...next] })
}

export function resetAgentTools(): void {
    write({ disabled: [] })
}

/** Tools for one agent mode that the user has left enabled. */
export function enabledAgentTools(mode: AgentMode): Set<string> {
    const disabled = disabledAgentTools()
    return new Set(
        BUILT_IN_AGENT_TOOLS.filter(
            (tool) =>
                (tool.mode === mode ||
                    (mode === 'assistant' &&
                        ['list_log_files', 'read_log_file', 'grep_log_file'].includes(tool.id))) &&
                !disabled.has(tool.id),
        ).map((tool) => tool.id),
    )
}

export function agentToolPermission(id: string): AgentToolPermission {
    return BUILT_IN_AGENT_TOOLS.find((tool) => tool.id === id)?.permission ?? 'read'
}
