export type AgentMode = 'assistant' | 'logs'

export type BuiltInToolDefinition = {
    id: string
    mode: AgentMode
}

/**
 * The tools the local agent can call. They are read-only and narrowly scoped,
 * so the settings page only lets the user turn them off rather than edit them.
 */
export const BUILT_IN_AGENT_TOOLS: BuiltInToolDefinition[] = [
    { id: 'get_instance_overview', mode: 'assistant' },
    { id: 'get_launcher_settings', mode: 'assistant' },
    { id: 'get_platform_diagnostics', mode: 'assistant' },
    { id: 'list_log_files', mode: 'logs' },
    { id: 'read_log_file', mode: 'logs' },
    { id: 'grep_log_file', mode: 'logs' },
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
        BUILT_IN_AGENT_TOOLS.filter((tool) => tool.mode === mode && !disabled.has(tool.id)).map(
            (tool) => tool.id,
        ),
    )
}
