export interface McpServerConfig {
    id: string
    name: string
    endpoint: string
}

export type McpServerConfigStorage = Pick<Storage, 'getItem' | 'setItem'>

export const MCP_SERVER_CONFIG_STORAGE_KEY = 'mctrin.assistant.mcp-servers.v1'

function normalizeConfig(value: unknown): McpServerConfig {
    if (!value || typeof value !== 'object') {
        throw new TypeError('MCP server configuration must be an object')
    }

    const candidate = value as Record<string, unknown>
    if (typeof candidate.id !== 'string' || !candidate.id.trim()) {
        throw new TypeError('MCP server id must be a non-empty string')
    }
    if (typeof candidate.name !== 'string') {
        throw new TypeError('MCP server name must be a string')
    }
    const name = candidate.name.trim()
    if (name.length < 1 || name.length > 64) {
        throw new TypeError('MCP server name must contain 1 to 64 characters')
    }
    if (typeof candidate.endpoint !== 'string') {
        throw new TypeError('MCP server endpoint must be a string')
    }

    return {
        id: candidate.id.trim(),
        name,
        endpoint: normalizeEndpoint(candidate.endpoint),
    }
}

function normalizeEndpoint(endpoint: string): string {
    const authorityMatch = endpoint.match(/^[a-z][a-z\d+.-]*:\/\/([^/?#]*)/i)
    if (!authorityMatch || endpoint.includes('#') || /\s/.test(endpoint)) {
        throw new TypeError('MCP server endpoint must be an absolute URL without a fragment')
    }

    const authority = authorityMatch[1]
    if (authority.includes('@')) {
        throw new TypeError('MCP server endpoint must not contain user information')
    }

    let url: URL
    try {
        url = new URL(endpoint)
    } catch {
        throw new TypeError('MCP server endpoint must be a valid URL')
    }

    if (url.username || url.password) {
        throw new TypeError('MCP server endpoint must not contain user information')
    }
    if (url.protocol !== 'https:' && url.protocol !== 'http:') {
        throw new TypeError('MCP server endpoint must use HTTPS or local HTTP')
    }

    if (url.protocol === 'http:') {
        const literalHost = authority.startsWith('[')
            ? authority.slice(0, authority.indexOf(']') + 1)
            : authority.split(':', 1)[0]
        if (!['localhost', '127.0.0.1', '[::1]'].includes(literalHost.toLowerCase())) {
            throw new TypeError('HTTP MCP endpoints are only allowed on localhost')
        }
    }

    return url.href
}

function ensureUniqueEndpoints(configs: McpServerConfig[]): void {
    const endpoints = new Set<string>()
    for (const config of configs) {
        if (endpoints.has(config.endpoint)) {
            throw new TypeError(`Duplicate MCP server endpoint: ${config.endpoint}`)
        }
        endpoints.add(config.endpoint)
    }
}

export function readMcpServerConfigs(
    storage: McpServerConfigStorage,
    key = MCP_SERVER_CONFIG_STORAGE_KEY,
): McpServerConfig[] {
    const raw = storage.getItem(key)
    if (raw === null) return []

    try {
        const parsed: unknown = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        const configs = parsed.map(normalizeConfig)
        ensureUniqueEndpoints(configs)
        return configs
    } catch {
        return []
    }
}

export function writeMcpServerConfigs(
    storage: McpServerConfigStorage,
    values: McpServerConfig[],
    key = MCP_SERVER_CONFIG_STORAGE_KEY,
): McpServerConfig[] {
    if (!Array.isArray(values)) {
        throw new TypeError('MCP server configurations must be an array')
    }
    const configs = values.map(normalizeConfig)
    ensureUniqueEndpoints(configs)
    storage.setItem(key, JSON.stringify(configs))
    return configs
}

export function upsertMcpServerConfig(
    storage: McpServerConfigStorage,
    value: McpServerConfig,
    key = MCP_SERVER_CONFIG_STORAGE_KEY,
): McpServerConfig[] {
    const config = normalizeConfig(value)
    const configs = readMcpServerConfigs(storage, key)
    const existingIndex = configs.findIndex((existing) => existing.id === config.id)
    const endpointOwner = configs.find(
        (existing) => existing.endpoint === config.endpoint && existing.id !== config.id,
    )
    if (endpointOwner) {
        throw new TypeError(`Duplicate MCP server endpoint: ${config.endpoint}`)
    }

    if (existingIndex === -1) configs.push(config)
    else configs[existingIndex] = config
    return writeMcpServerConfigs(storage, configs, key)
}

export function removeMcpServerConfig(
    storage: McpServerConfigStorage,
    id: string,
    key = MCP_SERVER_CONFIG_STORAGE_KEY,
): McpServerConfig[] {
    const configs = readMcpServerConfigs(storage, key)
    const remaining = configs.filter((config) => config.id !== id)
    if (remaining.length !== configs.length) writeMcpServerConfigs(storage, remaining, key)
    return remaining
}
