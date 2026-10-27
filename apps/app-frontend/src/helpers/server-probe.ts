import type { ServerData, World } from './worlds'

export type ServerProbeState = 'checking' | 'online' | 'offline' | 'error'

export function serverProbeState(data?: ServerData): ServerProbeState {
    if (!data || data.refreshing) return 'checking'
    if (data.status) return 'online'
    return data.probeState ?? 'offline'
}

export function classifyServerProbeError(error: unknown): 'offline' | 'error' {
    const message = error instanceof Error ? error.message : String(error)
    return /timed?\s*out|timeout|connection refused|connection reset|network is unreachable|no route to host|could not resolve|name or service not known|no such host|failed to (?:lookup|resolve)/i.test(
        message,
    )
        ? 'offline'
        : 'error'
}

export function pruneServerData(serverData: Record<string, ServerData>, worlds: World[]) {
    const activeAddresses = new Set(
        worlds.filter((world) => world.type === 'server').map((world) => world.address),
    )
    for (const address of Object.keys(serverData)) {
        if (!activeAddresses.has(address)) Reflect.deleteProperty(serverData, address)
    }
}
