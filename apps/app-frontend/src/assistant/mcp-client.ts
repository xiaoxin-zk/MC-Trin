export type McpToolDefinition = {
    name: string
    description?: string
    inputSchema?: Record<string, unknown>
}

type JsonRpcResponse = {
    id?: number | string
    result?: unknown
    error?: { code?: number; message?: string; data?: unknown }
}

export type McpFetch = (url: string, init: RequestInit) => Promise<Response>

const MAX_RESPONSE_BYTES = 2 * 1024 * 1024
const MAX_TOOLS = 64

function parseEndpoint(value: string): URL {
    let endpoint: URL
    try {
        endpoint = new URL(value.trim())
    } catch {
        throw new Error('Enter a valid MCP server URL.')
    }
    if (!['http:', 'https:'].includes(endpoint.protocol))
        throw new Error('MCP server URLs must use HTTP or HTTPS.')
    if (endpoint.username || endpoint.password)
        throw new Error('Store the MCP token in the token field, not in the URL.')
    if (endpoint.search)
        throw new Error(
            'MCP server URLs cannot include query parameters; use the token field for authorization.',
        )
    if (endpoint.hash) throw new Error('MCP server URLs cannot include a fragment.')
    return endpoint
}

function parseEventStream(body: string, expectedId: number): JsonRpcResponse {
    const responses = body.split(/\r?\n\r?\n/).flatMap((event) => {
        const data = event
            .split(/\r?\n/)
            .filter((line) => line.startsWith('data:'))
            .map((line) => line.slice(5).trim())
            .join('\n')
        if (!data) return []
        try {
            return [JSON.parse(data) as JsonRpcResponse]
        } catch {
            return []
        }
    })
    const response = responses.find((item) => item.id === expectedId)
    if (response) return response
    throw new Error('MCP event stream did not contain the matching response.')
}

export class StreamableHttpMcpClient {
    readonly endpoint: string
    private readonly token: string
    private readonly fetcher: McpFetch
    private sessionId: string | undefined
    private protocolVersion: string | undefined
    private nextId = 1
    private toolDefinitions: McpToolDefinition[] = []

    constructor(endpoint: string, token: string, fetcher: McpFetch) {
        this.endpoint = parseEndpoint(endpoint).toString()
        this.token = token.trim()
        this.fetcher = fetcher
    }

    get tools(): readonly McpToolDefinition[] {
        return this.toolDefinitions
    }

    private redact(value: string): string {
        return this.token ? value.replaceAll(this.token, '[redacted]') : value
    }

    async connect(signal?: AbortSignal): Promise<readonly McpToolDefinition[]> {
        this.sessionId = undefined
        this.protocolVersion = undefined
        this.nextId = 1
        this.toolDefinitions = []

        const initialized = (await this.request(
            'initialize',
            {
                protocolVersion: '2025-03-26',
                capabilities: {},
                clientInfo: { name: 'MC-Trin', version: '1.0.0' },
            },
            signal,
        )) as { protocolVersion?: unknown } | null
        this.protocolVersion =
            typeof initialized?.protocolVersion === 'string'
                ? initialized.protocolVersion
                : '2025-03-26'
        await this.notify('notifications/initialized', {}, signal)

        const names = new Set<string>()
        const cursors = new Set<string>()
        let cursor: string | undefined
        do {
            const result = (await this.request('tools/list', cursor ? { cursor } : {}, signal)) as {
                tools?: unknown
                nextCursor?: unknown
            }
            if (!Array.isArray(result?.tools))
                throw new Error('MCP server returned an invalid tool list.')
            if (this.toolDefinitions.length + result.tools.length > MAX_TOOLS)
                throw new Error(`MCP server returned more than ${MAX_TOOLS} tools.`)

            result.tools.forEach((item, index) => {
                if (!item || typeof item !== 'object')
                    throw new Error(`MCP tool ${index + 1} is not an object.`)
                const tool = item as Record<string, unknown>
                if (typeof tool.name !== 'string' || !tool.name.trim() || tool.name.length > 128)
                    throw new Error(`MCP tool ${index + 1} has an invalid name.`)
                if (names.has(tool.name))
                    throw new Error(`MCP server has duplicate tool name: ${tool.name}`)
                names.add(tool.name)
                if (
                    tool.inputSchema !== undefined &&
                    (!tool.inputSchema ||
                        typeof tool.inputSchema !== 'object' ||
                        Array.isArray(tool.inputSchema))
                ) {
                    throw new Error(`MCP tool ${tool.name} has an invalid input schema.`)
                }
                this.toolDefinitions.push({
                    name: tool.name,
                    description:
                        typeof tool.description === 'string' ? tool.description.slice(0, 2000) : '',
                    inputSchema: tool.inputSchema as Record<string, unknown> | undefined,
                })
            })

            if (
                result.nextCursor !== undefined &&
                (typeof result.nextCursor !== 'string' || result.nextCursor.length > 512)
            ) {
                throw new Error('MCP server returned an invalid tools/list cursor.')
            }
            cursor = typeof result.nextCursor === 'string' ? result.nextCursor : undefined
            if (cursor) {
                if (cursors.has(cursor)) throw new Error('MCP server repeated a tools/list cursor.')
                cursors.add(cursor)
            }
        } while (cursor)
        return this.tools
    }

    async callTool(
        name: string,
        args: Record<string, unknown>,
        signal?: AbortSignal,
    ): Promise<unknown> {
        if (!this.toolDefinitions.some((tool) => tool.name === name))
            throw new Error('MCP tool is not in the connected server tool list.')
        if (JSON.stringify(args).length > 64 * 1024)
            throw new Error('MCP tool arguments are too large.')
        const result = (await this.request('tools/call', { name, arguments: args }, signal)) as {
            isError?: unknown
            content?: unknown
        } | null
        if (result?.isError === true) {
            const detail = Array.isArray(result.content)
                ? result.content
                      .filter(
                          (item): item is { text: string } =>
                              Boolean(item) &&
                              typeof item === 'object' &&
                              typeof (item as { text?: unknown }).text === 'string',
                      )
                      .map((item) => item.text)
                      .join('\n')
                : ''
            throw new Error(this.redact(detail.slice(0, 1000) || `MCP tool ${name} failed.`))
        }
        return result
    }

    private async notify(
        method: string,
        params: Record<string, unknown>,
        signal?: AbortSignal,
    ): Promise<void> {
        const response = await this.post({ jsonrpc: '2.0', method, params }, signal)
        if (response.status === 202 || response.status === 204) return
        const body = await response.text()
        if (body.length > MAX_RESPONSE_BYTES) throw new Error('MCP response is too large.')
    }

    private async request(
        method: string,
        params: Record<string, unknown>,
        signal?: AbortSignal,
    ): Promise<unknown> {
        const id = this.nextId++
        const response = await this.post({ jsonrpc: '2.0', id, method, params }, signal)
        if (response.status === 202 || response.status === 204)
            throw new Error(`MCP server accepted ${method} without returning a response.`)
        const body = await response.text()
        if (new TextEncoder().encode(body).byteLength > MAX_RESPONSE_BYTES)
            throw new Error('MCP response is too large.')
        const responseType = response.headers.get('content-type') ?? ''
        let message: JsonRpcResponse
        if (responseType.includes('text/event-stream')) {
            message = parseEventStream(body, id)
        } else {
            try {
                message = JSON.parse(body) as JsonRpcResponse
            } catch {
                throw new Error('MCP server returned an invalid JSON response.')
            }
        }
        if (message.id !== id) throw new Error('MCP response ID did not match the request.')
        if (message.error)
            throw new Error(
                this.redact((message.error.message || 'MCP request failed.').slice(0, 1000)),
            )
        if (!Object.hasOwn(message, 'result'))
            throw new Error('MCP response did not include a result.')
        return message.result
    }

    private async post(message: Record<string, unknown>, signal?: AbortSignal): Promise<Response> {
        const headers = new Headers({
            Accept: 'application/json, text/event-stream',
            'Content-Type': 'application/json',
        })
        if (this.sessionId) headers.set('Mcp-Session-Id', this.sessionId)
        if (this.protocolVersion) headers.set('MCP-Protocol-Version', this.protocolVersion)
        if (this.token) headers.set('Authorization', `Bearer ${this.token}`)
        const requestSignal = signal ?? AbortSignal.timeout(30_000)
        const response = await this.fetcher(this.endpoint, {
            method: 'POST',
            headers,
            body: JSON.stringify(message),
            signal: requestSignal,
        })
        const sessionId = response.headers.get('Mcp-Session-Id')
        if (sessionId) this.sessionId = sessionId
        if (!response.ok && response.status !== 202) throw new Error(`MCP HTTP ${response.status}`)
        return response
    }
}
