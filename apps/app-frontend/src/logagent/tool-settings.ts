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
 * Local tools that change launcher data.
 *
 * Mirrors `assistant_write_tool` in
 * `packages/app-lib/src/api/logs/logagent/mod.rs`. The Rust gate refuses an
 * unapproved write, so a name that exists there but not here would fail after
 * the model already decided to act; the parity test
 * `apps/app/tests/logagent_write_tool_parity.rs` fails the build instead.
 */
export const ASSISTANT_WRITE_TOOLS: ReadonlySet<string> = new Set([
    'update_launcher_settings',
    'create_instance',
    'rename_instance',
    'delete_instance',
    'install_java_runtime',
    'set_java_default_runtime',
    'queue_resource_install',
    'cancel_install_job',
    'disable_content',
    'enable_content',
    'remove_content',
])

/**
 * The local tools the narrower permission modes keep asking about. Every write
 * tool always asks, so this is exactly the write list: the model is told that
 * every write needs explicit approval, and free mode must not bypass that.
 */
export const DANGEROUS_AGENT_TOOLS: ReadonlySet<string> = ASSISTANT_WRITE_TOOLS

/**
 * The local tools are deliberately narrow. Read tools expose sanitized launcher
 * context; write tools are limited to launcher settings, Java installation,
 * resource jobs, and instance content lifecycle actions. Every write tool
 * requires an explicit approval before the call in the default and free modes;
 * only full access, which carries its own warning, runs them without asking.
 */
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

/**
 * Whether a tool call must go through the approval dialog.
 *
 * `full` runs everything directly. Every other mode asks for remote tools,
 * because this launcher does not control what those servers do, and for every
 * local write: the assistant prompt tells the model each write needs explicit
 * approval, so no mode below full may silently grant one.
 */
export function toolNeedsApproval(
    mode: AssistantPermissionMode,
    toolName: string,
    local: boolean,
): boolean {
    if (mode === 'full') return false
    if (mode === 'default') return true
    return !local || DANGEROUS_AGENT_TOOLS.has(toolName)
}
