/** A room that needs no lobby service: both sides agree on a name and a
 * password, and no directory ever learns the room exists. */
export interface DirectRoomRequest {
    room_name: string
    password: string
    player_name: string
    server_node: string
    game_port: number
    /** Host virtual address, present only when joining an existing room. */
    host_address?: string | null
}

export interface MctierStartRequest {
    mode: 'create' | 'join'
    lobby_name: string
    password: string
    player_name: string
    server_node: string
    signaling_server: string
    game_port: number
    is_public?: boolean
    description?: string | null
    max_players?: number | null
}

export interface MctierInvite {
    lobby_name: string
    password: string
    server_node: string
    signaling_server: string
}

export interface MctierPlayer {
    playerId: string
    playerName: string
    virtualIp: string | null
    sessionGeneration: number
}

export type MctierPreparationStage = 'downloading' | 'extracting' | 'verifying'

/** How the host's public listing request resolved; mirrors the Rust enum. */
export type MctierPublishOutcome = 'private' | 'published' | 'sent_unconfirmed' | 'not_listed'

export interface MctierState {
    status: 'idle' | 'downloading' | 'connecting' | 'hosting' | 'joined' | 'error'
    mode: 'create' | 'join' | null
    lobby_name: string | null
    virtual_ip: string | null
    game_port: number | null
    connect_address: string | null
    players: MctierPlayer[]
    local_player_id: string | null
    voice_ice_server: string | null
    error_message: string | null
    is_public: boolean
    publish_outcome: MctierPublishOutcome
    max_players: number | null
    has_password: boolean
    download_stage: MctierPreparationStage | null
    download_progress: number | null
    host_hint: 'game_port_closed' | null
    /** Which provider owns the running session, so a tab cannot show the other
     * provider's room as its own. Absent only when no session exists. */
    room_kind?: MctierRoomKind | null
}

export type MctierRoomKind = 'lobby' | 'direct'

/** What a pasted direct-room code resolved to. The host address is the only
 * required part; the rest is filled in from the code when it carries it. */
export interface ParsedDirectRoom {
    host_address: string
    room_name?: string
    server_node?: string
    game_port?: number
}

export const DEFAULT_MCTIER_SIGNALING = 'wss://mctier.pmhs.top/signaling'
export const DEFAULT_MCTIER_NODE = 'tcp://easytier.weiai.org.cn:11010'

function validServerNode(value: string): boolean {
    const match = /^([a-z][a-z0-9+.-]*):\/\//i.exec(value)
    if (!match || !['tcp', 'udp', 'ws', 'wss'].includes(match[1].toLowerCase())) return false
    try {
        const parsed = new URL(`http://${value.slice(match[0].length)}`)
        return Boolean(parsed.hostname) && !parsed.username && !parsed.password && !parsed.hash
    } catch {
        return false
    }
}

function serverNodeValue(value: string): string | null {
    const trimmed = value.trim()
    return validServerNode(trimmed) ? trimmed : null
}

const DIRECT_ROOM_CODE_PATTERN = /^mctrin-direct:\/\/room\/?\?(\S+)$/i

/**
 * Reads the code the direct room shows its host.
 *
 * Only `host` is mandatory: a code may have been shared before the room
 * existed or be typed by hand. Anything malformed is rejected as a whole rather
 * than silently joining a different room than the code named.
 */
export function parseDirectRoomCode(value: string): ParsedDirectRoom | null {
    const trimmed = value.trim()
    if (!trimmed) return null

    const query = DIRECT_ROOM_CODE_PATTERN.exec(trimmed)?.[1]
    if (query) {
        let params: URLSearchParams
        try {
            params = new URLSearchParams(query)
        } catch {
            return null
        }
        const host = params.get('host')?.trim() ?? ''
        // The backend dials an overlay IPv4 address only; a hostname here would
        // pass this parser and fail validation after the download.
        if (!isIpv4(host)) return null
        const name = params.get('name')?.trim() ?? ''
        if (name && !/^[\p{L}\p{N}_-]{4,32}$/u.test(name)) return null
        const node = params.get('node')?.trim() ?? ''
        const serverNode = node ? serverNodeValue(node) : null
        if (node && !serverNode) return null
        const rawPort = params.get('port')?.trim() ?? ''
        const gamePort = rawPort ? localPort(rawPort) : null
        if (rawPort && gamePort === null) return null
        return {
            host_address: host,
            ...(name ? { room_name: name } : {}),
            ...(serverNode ? { server_node: serverNode } : {}),
            ...(gamePort !== null ? { game_port: gamePort } : {}),
        }
    }

    // A bare address is what a host reads out loud, so accept it too.
    return isIpv4(trimmed) ? { host_address: trimmed } : null
}

function isIpv4(value: string): boolean {
    const parts = value.split('.')
    return (
        parts.length === 4 &&
        parts.every(
            (part) => /^\d{1,3}$/.test(part) && Number(part) <= 255 && String(Number(part)) === part,
        )
    )
}

function localPort(value: string): number | null {
    if (!/^\d+$/.test(value)) return null
    const port = Number(value)
    return Number.isInteger(port) && port >= 1 && port <= 65535 ? port : null
}

/** Validate the EasyTier node the direct-room form sends to the kernel. */
export function isValidDirectServerNode(value: string): boolean {
    return serverNodeValue(value) !== null
}

function validSignalingServer(value: string): boolean {
    try {
        const parsed = new URL(value)
        const local = ['localhost', '127.0.0.1', '[::1]'].includes(parsed.hostname)
        return (
            (parsed.protocol === 'wss:' || (parsed.protocol === 'ws:' && local)) &&
            !parsed.username &&
            !parsed.password &&
            !parsed.hash
        )
    } catch {
        return false
    }
}

export function parseMctierInviteCode(value: string): MctierInvite | null {
    const link = value.trim().match(/(?:mctrin|mctier):\/\/join\/?\?([^\s]+)/i)?.[0]
    if (!link) return null

    let params: URLSearchParams
    try {
        const query = link.match(/^(?:mctrin|mctier):\/\/join\/?\?([^\s]+)$/i)?.[1]
        if (!query) return null
        params = new URLSearchParams(query)
    } catch {
        return null
    }

    const version = params.get('v')
    if (version && !['1', '2', '3'].includes(version)) return null
    const lobbyName = params.get('name')?.trim() ?? ''
    if (!/^[\p{L}\p{N}_-]{4,32}$/u.test(lobbyName)) return null

    const secret = params.get('secret') ?? ''
    if (
        version === '3' &&
        (secret.length > 4096 || (secret && !/^mctier-invite-v3:[A-Za-z0-9_-]+$/.test(secret)))
    ) {
        return null
    }
    const rawNode = params.get('node')?.trim() ?? ''
    const rawSignaling = params.get('signal')?.trim() ?? ''
    if (rawNode && !validServerNode(rawNode)) return null
    if (rawSignaling && !validSignalingServer(rawSignaling)) return null

    return {
        lobby_name: lobbyName,
        password: version === '3' ? secret : (params.get('pwd') ?? ''),
        server_node: rawNode || DEFAULT_MCTIER_NODE,
        signaling_server: rawSignaling || DEFAULT_MCTIER_SIGNALING,
    }
}
