export type AnalysisSource = 'custom' | 'logshare' | 'assistant'
export type LogAnalysisSource = 'custom' | 'logshare'

export interface AnalysisSettings {
    enabled: boolean
    provider_id: string
    model_id: string
    ai_source: LogAnalysisSource
}

export interface ToolCall {
    id: string
    name: string
    args: Record<string, unknown>
}

export interface ModelMessage {
    role: string
    content: string
    tool_calls?: ToolCall[]
    tool_call_id?: string
    opaque?: unknown
}

export interface ModelTurn {
    messages: ModelMessage[]
    tools: ModelToolDefinition[]
}

export interface ModelToolDefinition {
    type: 'function'
    function: {
        name: string
        description?: string
        parameters: unknown
    }
}

export interface PreparedSession {
    run_id: string
    provider_id: string
    model_id: string
    locale: string
    context: string
}

export interface AnalysisEvent {
    run_id: string
    event_type: string
    data: Record<string, unknown>
}

export interface UploadTicket {
    id: string
    url: string
    raw: string
    token: string
}

export interface ToolRequest {
    name: string
    file_id?: string
    start_line?: number
    end_line?: number
    query?: string
    context?: number
    instance_id?: string
    instance_name?: string
    game_version?: string
    loader?: string
    loader_version?: string
    patch?: Record<string, unknown>
    java_version?: number
    vendor?: string
    source?: string
    project_id?: string
    version_id?: string
    resource_type?: string
    job_id?: string
    plan_id?: string
    plan_revision?: number
    content_id?: string
    enabled?: boolean
    install_dependencies?: boolean
}

export interface AgentToolApproval {
    name: string
    description?: string
    permission: 'write'
}

export interface RemoteMcpTool {
    name: string
    description?: string
    inputSchema?: Record<string, unknown>
}

export interface RemoteMcpGateway {
    endpoint: string
    tools: readonly RemoteMcpTool[]
    approveTool(tool: RemoteMcpTool, args: Record<string, unknown>): Promise<boolean>
    callTool(name: string, args: Record<string, unknown>, signal: AbortSignal): Promise<unknown>
}

export interface AgentGateway {
    modelTurn(turn: ModelTurn, onEvent?: (event: ModelStreamEvent) => void): Promise<ModelMessage>
    tool(request: ToolRequest): Promise<unknown>
}

export interface ModelStreamEvent {
    event_type: string
    data: Record<string, unknown>
}

export interface LocalProgress {
    event_type: 'model_start' | 'model_end' | 'delta' | 'thinking' | 'tool' | 'tool_result'
    data: Record<string, unknown>
}
