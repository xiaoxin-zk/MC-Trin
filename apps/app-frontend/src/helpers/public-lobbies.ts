import type { MctierStartRequest } from '@/helpers/mctier'

export const PUBLIC_LOBBY_SIGNALING_URL = 'wss://mctier.pmhs.top/signaling'
export const DEFAULT_PUBLIC_LOBBY_NODE = 'tcp://easytier.weiai.org.cn:11010'

export interface PublicLobby {
    lobbyName: string
    hostName: string
    playerCount: number
    maxPlayers: number | null
    description: string
    /** The host's EasyTier node, when the signaling directory provides one. */
    serverNode?: string
}

const signalingUrl = PUBLIC_LOBBY_SIGNALING_URL
const maxResponseLength = 500_000
const maxLobbyCount = 300
const maxPlayerCount = 100_000
const serverNodeProtocols = new Set(['tcp:', 'udp:', 'ws:', 'wss:', 'txt:'])

function isSafeServerNode(value: unknown): value is string {
    if (typeof value !== 'string') return false
    const trimmed = value.trim()
    const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(trimmed)
    if (!scheme || !serverNodeProtocols.has(`${scheme[1].toLowerCase()}:`)) return false
    const remainder = trimmed.slice(scheme[0].length)
    if (!remainder || /^[/?#]/.test(remainder) || remainder.includes('\\')) return false
    try {
        const parsed = new URL(`http://${remainder}`)
        return Boolean(parsed.hostname) && !parsed.username && !parsed.password && !parsed.hash
    } catch {
        return false
    }
}

function cleanText(value: unknown, limit: number): string {
    if (typeof value !== 'string') return ''
    return value
        .replace(/[\p{Cc}\p{Cf}]/gu, '')
        .slice(0, limit)
        .trim()
}

function hasControlCharacters(value: string): boolean {
    return Array.from(value).some((character) => {
        const code = character.charCodeAt(0)
        return (code >= 0 && code <= 31) || code === 127
    })
}

function normalizeCount(value: unknown, minimum: number): number | null {
    if (typeof value !== 'number' || !Number.isFinite(value)) return null
    return Math.max(minimum, Math.min(maxPlayerCount, Math.floor(value)))
}

function parseLobby(value: unknown): PublicLobby | null {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null

    const item = value as Record<string, unknown>
    const lobbyName = cleanText(item.lobbyName, 64)
    const hostName = cleanText(item.hostName, 64)
    if (!lobbyName || !hostName) return null

    return {
        lobbyName,
        hostName,
        playerCount: normalizeCount(item.playerCount, 0) ?? 0,
        maxPlayers: normalizeCount(item.maxPlayers, 1),
        description: cleanText(item.description ?? item.desc, 200),
        ...(isSafeServerNode(item.serverNode) && item.serverNode.trim() !== 'custom'
            ? { serverNode: item.serverNode.trim() }
            : {}),
    }
}

/**
 * Build the only join request a public directory entry is allowed to create.
 * The directory intentionally does not provide a password or Minecraft port;
 * both remain explicit user input instead of being guessed or copied from
 * untrusted listing data.
 */
export function buildPublicLobbyJoinRequest(
    lobby: PublicLobby,
    playerName: string,
    gamePort: number,
    password = '',
): MctierStartRequest {
    const name = lobby.lobbyName.trim()
    const player = playerName.trim()
    const secret = password.trim()
    if (!/^[\p{L}\p{N}_-]{4,32}$/u.test(name)) {
        throw new Error('The public lobby name is invalid')
    }
    if (!player || player.length > 64 || hasControlCharacters(player)) {
        throw new Error('Enter a valid player name')
    }
    if (!Number.isInteger(gamePort) || gamePort < 1 || gamePort > 65535) {
        throw new Error('Minecraft port must be between 1 and 65535')
    }
    if (secret.length > 128 || hasControlCharacters(secret)) {
        throw new Error('The lobby password is invalid')
    }
    const serverNode = lobby.serverNode ?? DEFAULT_PUBLIC_LOBBY_NODE
    if (!isSafeServerNode(serverNode)) {
        throw new Error('The public lobby node is invalid')
    }
    return {
        mode: 'join',
        lobby_name: name,
        password: secret,
        player_name: player,
        server_node: serverNode,
        signaling_server: PUBLIC_LOBBY_SIGNALING_URL,
        game_port: gamePort,
    }
}

/** Fetches the public directory only; connection details and credentials are not exposed. */
export function fetchPublicLobbies(
    timeoutMs = 8_000,
    signal?: AbortSignal,
): Promise<PublicLobby[]> {
    return new Promise((resolve, reject) => {
        if (signal?.aborted) {
            reject(new DOMException('Public lobby request cancelled', 'AbortError'))
            return
        }

        let socket: WebSocket
        try {
            socket = new WebSocket(signalingUrl)
        } catch {
            reject(new Error('Could not load public lobbies'))
            return
        }

        let settled = false
        const abort = () => finish(new DOMException('Public lobby request cancelled', 'AbortError'))
        const timeout = setTimeout(() => {
            finish(new Error('Public lobby request timed out'))
        }, timeoutMs)

        function finish(error?: Error, lobbies?: PublicLobby[]) {
            if (settled) return
            settled = true
            clearTimeout(timeout)
            signal?.removeEventListener('abort', abort)
            socket.onopen = null
            socket.onmessage = null
            socket.onerror = null
            socket.onclose = null
            socket.close()
            if (error) reject(error)
            else resolve(lobbies ?? [])
        }

        socket.onopen = () => {
            try {
                socket.send(JSON.stringify({ type: 'public-lobby-list-request' }))
            } catch {
                finish(new Error('Could not load public lobbies'))
            }
        }

        socket.onmessage = (event) => {
            if (typeof event.data !== 'string') return
            if (event.data.length > maxResponseLength) {
                finish(new Error('Public lobby response was too large'))
                return
            }

            let message: unknown
            try {
                message = JSON.parse(event.data)
            } catch {
                return
            }

            if (!message || typeof message !== 'object' || Array.isArray(message)) return
            const response = message as Record<string, unknown>
            if (response.type !== 'public-lobby-list-response') return
            if (!Array.isArray(response.lobbies)) {
                finish(new Error('Public lobby response was invalid'))
                return
            }

            const lobbies = response.lobbies
                .slice(0, maxLobbyCount)
                .map(parseLobby)
                .filter((lobby): lobby is PublicLobby => lobby !== null)
            finish(undefined, lobbies)
        }

        socket.onerror = () => finish(new Error('Could not load public lobbies'))
        socket.onclose = () => {
            if (!settled) finish(new Error('Could not load public lobbies'))
        }
        signal?.addEventListener('abort', abort, { once: true })
    })
}
