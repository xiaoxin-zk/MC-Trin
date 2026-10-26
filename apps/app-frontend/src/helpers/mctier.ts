export interface MctierStartRequest {
    mode: 'create' | 'join'
    lobby_name: string
    password: string
    player_name: string
    server_node: string
    signaling_server: string
    game_port: number
    is_public?: boolean
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
    has_password: boolean
    download_stage: MctierPreparationStage | null
    download_progress: number | null
    host_hint: 'game_port_closed' | null
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
