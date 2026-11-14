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

/** Rust compares `value.len()` against 2048, which counts UTF-8 bytes. */
const MAX_ENDPOINT_BYTES = 2048
/** `char::is_control` in Rust is the Unicode Cc category. Written as a code
 * point comparison so the rule does not embed literal control characters. */
function hasControlCharacter(value: string): boolean {
    for (const character of value) {
        const code = character.codePointAt(0) ?? 0
        if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true
    }
    return false
}

/** Mirrors the length and control-character precheck shared by both endpoint
 * rules in `validate_endpoint`, so neither validator can accept text the
 * backend rejects before it even parses the URL. */
function endpointTextIsUsable(value: string): boolean {
    return (
        new TextEncoder().encode(value).length <= MAX_ENDPOINT_BYTES && !hasControlCharacter(value)
    )
}

function endpointScheme(parsed: URL): string {
    return parsed.protocol.replace(/:$/, '').toLowerCase()
}

/** Mirrors `validate_endpoint(_, false)` in `mctier.rs`, the rule every
 * node/peer address is held to. The backend rejects paths and queries that this
 * used to wave through, which only surfaced after the kernel was already
 * started. `port_or_known_default` counts the ws/wss defaults, so only tcp/udp
 * must spell the port out. */
function validServerNode(value: string): boolean {
    if (!endpointTextIsUsable(value)) return false
    let parsed: URL
    try {
        parsed = new URL(value)
    } catch {
        return false
    }
    const scheme = endpointScheme(parsed)
    if (!['tcp', 'udp', 'ws', 'wss'].includes(scheme)) return false
    if (!parsed.hostname) return false
    if (parsed.username || parsed.password || value.includes('?') || value.includes('#'))
        return false
    if (parsed.pathname !== '' && parsed.pathname !== '/') return false
    return parsed.port !== '' || scheme === 'ws' || scheme === 'wss'
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
            (part) =>
                /^\d{1,3}$/.test(part) && Number(part) <= 255 && String(Number(part)) === part,
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

/** Rust's `char::is_alphanumeric` covers letters and every numeric category. */
function isAlphanumericCharacter(character: string): boolean {
    return /[\p{L}\p{N}]/u.test(character)
}

/**
 * Mirrors the lobby-name rule in `StartRequest::validate`.
 *
 * A name made only of underscores or hyphens used to pass here and fail in the
 * backend, so the form now requires at least one letter or number. Lengths are
 * counted in code points, not UTF-16 units, because Rust counts `char`s.
 */
export function isValidMctierLobbyName(value: string): boolean {
    const characters = [...value]
    if (characters.length < 4 || characters.length > 32) return false
    if (characters.some(hasControlCharacter)) return false
    if (value.trim() !== value) return false
    if (!characters.some(isAlphanumericCharacter)) return false
    return characters.every(
        (character) => isAlphanumericCharacter(character) || character === '_' || character === '-',
    )
}

/**
 * Mirrors the player-name rule in `StartRequest::validate`: the caller submits a
 * trimmed name, so 1-64 code points of that value, without control characters.
 */
export function isValidMctierPlayerName(value: string): boolean {
    const characters = [...value.trim()]
    return (
        characters.length >= 1 && characters.length <= 64 && !characters.some(hasControlCharacter)
    )
}

/** Mirrors `validate_endpoint(_, true)`, the signalling rule. Signalling keeps
 * its path and query (the service lives at e.g. `/signaling?token=...`), so only
 * credentials and the fragment are refused. */
function validSignalingServer(value: string): boolean {
    if (!endpointTextIsUsable(value)) return false
    let parsed: URL
    try {
        parsed = new URL(value)
    } catch {
        return false
    }
    const scheme = endpointScheme(parsed)
    const local = ['localhost', '127.0.0.1', '[::1]'].includes(parsed.hostname)
    if (scheme !== 'wss' && !(scheme === 'ws' && local)) return false
    return Boolean(parsed.hostname) && !parsed.username && !parsed.password && !value.includes('#')
}

/** Whether the lobby service address may be sent to the backend. */
export function isValidMctierSignalingServer(value: string): boolean {
    return validSignalingServer(value.trim())
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
