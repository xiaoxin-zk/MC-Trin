export interface AgentProfile {
    id: string
    name: string
    instructions: string
}

export type AIAgentProfile = AgentProfile

export type AgentProfileStorage = Pick<Storage, 'getItem' | 'setItem'>

export const AI_AGENT_PROFILES_STORAGE_KEY = 'mc-trin:ai-agent-profiles:v1'
export const DEFAULT_AI_AGENT_PROFILE: Readonly<AgentProfile> = Object.freeze({
    id: 'mc-trin-assistant',
    name: 'MC-Trin 助手',
    instructions:
        '你是 MC-Trin 的 Minecraft 启动器助手。使用简体中文回答，帮助用户排查启动失败、Java 与游戏版本兼容、模组和整合包安装、下载源与网络问题。先根据用户提供的日志和环境信息定位原因，再给出可操作且有顺序的解决步骤；不确定时明确说明并询问必要信息。涉及删除文件、覆盖存档或修改系统设置时，先解释影响并要求用户确认。',
})

const maxNameLength = 40
const maxInstructionsLength = 2000
const maxIdLength = 128

function defaultProfiles(): AgentProfile[] {
    return [{ ...DEFAULT_AI_AGENT_PROFILE }]
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeProfile(value: unknown): AgentProfile | null {
    if (!isRecord(value)) return null

    const { id, name, instructions } = value
    if (typeof id !== 'string' || id.trim().length === 0 || id.length > maxIdLength) return null
    if (
        typeof name !== 'string' ||
        name.trim().length === 0 ||
        Array.from(name.trim()).length > maxNameLength
    ) {
        return null
    }
    if (typeof instructions !== 'string' || Array.from(instructions).length > maxInstructionsLength)
        return null

    return { id, name: name.trim(), instructions }
}

function readProfiles(storage: AgentProfileStorage): AgentProfile[] {
    let serialized: string | null
    try {
        serialized = storage.getItem(AI_AGENT_PROFILES_STORAGE_KEY)
    } catch {
        return defaultProfiles()
    }

    if (serialized === null) return defaultProfiles()

    let parsed: unknown
    try {
        parsed = JSON.parse(serialized)
    } catch {
        return defaultProfiles()
    }

    if (!Array.isArray(parsed)) return defaultProfiles()

    const profiles = defaultProfiles()
    const seenIds = new Set([DEFAULT_AI_AGENT_PROFILE.id])

    for (const candidate of parsed) {
        const profile = normalizeProfile(candidate)
        if (!profile) continue

        if (profile.id === DEFAULT_AI_AGENT_PROFILE.id) {
            profiles[0] = profile
            continue
        }
        if (seenIds.has(profile.id)) continue

        seenIds.add(profile.id)
        profiles.push(profile)
    }

    return profiles
}

function saveProfiles(storage: AgentProfileStorage, profiles: AgentProfile[]): void {
    storage.setItem(AI_AGENT_PROFILES_STORAGE_KEY, JSON.stringify(profiles))
}

function validateDraft(profile: Omit<AgentProfile, 'id'>): Omit<AgentProfile, 'id'> {
    const normalized = normalizeProfile({ id: 'validation-only', ...profile })
    if (!normalized) {
        throw new TypeError(
            'Agent profile name must be 1-40 characters and instructions must be at most 2000 characters.',
        )
    }

    return { name: normalized.name, instructions: normalized.instructions }
}

export function getAgentProfiles(storage: AgentProfileStorage): AgentProfile[] {
    return readProfiles(storage)
}

export function addAgentProfile(
    storage: AgentProfileStorage,
    profile: Omit<AgentProfile, 'id'>,
): AgentProfile[] {
    const draft = validateDraft(profile)
    const profiles = readProfiles(storage)
    let id = globalThis.crypto.randomUUID()
    while (profiles.some((existing) => existing.id === id)) {
        id = globalThis.crypto.randomUUID()
    }

    const updated = [...profiles, { id, ...draft }]
    saveProfiles(storage, updated)
    return updated
}

export function updateAgentProfile(
    storage: AgentProfileStorage,
    id: string,
    updates: Partial<Omit<AgentProfile, 'id'>>,
): AgentProfile[] {
    const profiles = readProfiles(storage)
    const index = profiles.findIndex((profile) => profile.id === id)
    if (index === -1) return profiles

    const current = profiles[index]
    const updatedProfile = normalizeProfile({
        id: current.id,
        name: Object.hasOwn(updates, 'name') ? updates.name : current.name,
        instructions: Object.hasOwn(updates, 'instructions')
            ? updates.instructions
            : current.instructions,
    })
    if (!updatedProfile) {
        throw new TypeError(
            'Agent profile name must be 1-40 characters and instructions must be at most 2000 characters.',
        )
    }

    const updated = profiles.map((profile, profileIndex) =>
        profileIndex === index ? updatedProfile : profile,
    )
    saveProfiles(storage, updated)
    return updated
}

export function removeAgentProfile(storage: AgentProfileStorage, id: string): AgentProfile[] {
    if (id === DEFAULT_AI_AGENT_PROFILE.id) return readProfiles(storage)

    const profiles = readProfiles(storage)
    const updated = profiles.filter((profile) => profile.id !== id)
    if (updated.length !== profiles.length) saveProfiles(storage, updated)
    return updated
}
