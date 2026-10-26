/** Local MC-Trin manifest format and the narrow sandbox bridge. No native APIs live here. */
export const LOCAL_PLUGINS_KEY = 'mctrin.plugins.v1'
export const MAX_MANIFEST_BYTES = 1_200_000
export const PLUGIN_PERMISSIONS = [
    'instances.read',
    'ui.toast',
    'ui.theme',
    'app.navigate',
    'game.launchDefault',
] as const
export const PLUGIN_ACCENTS = ['emerald', 'ocean', 'violet', 'amber', 'rose'] as const
export const PLUGIN_TARGETS = {
    home: '/',
    versions: '/library',
    download: '/browse/modpack',
    multiplayer: '/multiplayer',
    ai: '/agent',
    plugins: '/plugins',
    settings: '/settings',
} as const

export type PluginPermission = (typeof PLUGIN_PERMISSIONS)[number]
export type PluginAccent = (typeof PLUGIN_ACCENTS)[number]
export type PluginTarget = keyof typeof PLUGIN_TARGETS
export type PluginTheme = { accent: PluginAccent; mode?: 'dark' | 'light' | 'oled' }
export type PluginAction = {
    id: string
    label: string
    type: 'toast' | 'theme' | 'navigate' | 'launchDefault'
    value?: string
}
export type PluginPage = { title: string; html: string; script?: string }
export type PluginPanel = PluginPage & { id: string; slot: 'home.after'; height: number }
export type LocalPluginManifest = {
    id: string
    name: string
    version: string
    description?: string
    permissions: PluginPermission[]
    actions: PluginAction[]
    page?: PluginPage
    theme?: PluginTheme
    panels: PluginPanel[]
}
export type InstalledLocalPlugin = { manifest: LocalPluginManifest; enabled: boolean }
export type PluginInstance = {
    id: string
    name: string
    gameVersion: string
    loader: string
    selected: boolean
}
export type PluginRequest = {
    type: 'mctrin:request'
    requestId: string
    session: string
    action: 'instances.list' | 'actions.run'
    actionId?: string
}
export type PluginResponse = {
    type: 'mctrin:response'
    requestId: string
    session: string
    ok: boolean
    value: unknown
}
export type PluginHost = {
    instances: () => Promise<PluginInstance[]>
    toast: (message: string) => void
    theme: (theme: PluginTheme) => Promise<void>
    navigate: (target: PluginTarget) => Promise<unknown>
    launchDefault: (pluginName: string) => Promise<boolean>
}

const permissionForAction: Record<PluginAction['type'], PluginPermission> = {
    toast: 'ui.toast',
    theme: 'ui.theme',
    navigate: 'app.navigate',
    launchDefault: 'game.launchDefault',
}

function object(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function requiredText(value: unknown, max: number, field: string): string {
    if (
        typeof value !== 'string' ||
        !value.trim() ||
        value.length > max ||
        [...value].some(
            (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
        )
    )
        throw new Error(`Invalid ${field}.`)
    return value.trim()
}

function page(value: unknown, field: string): PluginPage {
    if (!object(value)) throw new Error(`Invalid ${field}.`)
    const title = requiredText(value.title, 80, `${field}.title`)
    if (typeof value.html !== 'string' || value.html.length > 500_000)
        throw new Error(`${field}.html must be text up to 500 KB.`)
    if (
        value.script !== undefined &&
        (typeof value.script !== 'string' || value.script.length > 100_000)
    )
        throw new Error(`${field}.script must be text up to 100 KB.`)
    // Scripts have one explicit field and one nonce. HTML cannot insert another executable
    // document, replace the CSP, submit forms, or declare inline event handlers.
    if (
        /<\s*\/?\s*(?:script|iframe|frame|frameset|object|embed|portal|webview|meta|base|link|form|html|head|body)\b/iu.test(
            value.html,
        )
    )
        throw new Error(`${field}.html contains an unsupported element.`)
    if (/\s(?:on[a-z0-9_-]*|srcdoc)\s*=/iu.test(value.html) || /javascript\s*:/iu.test(value.html))
        throw new Error(`${field}.html cannot contain event handlers or executable URLs.`)
    return {
        title,
        html: value.html,
        ...(value.script === undefined ? {} : { script: value.script as string }),
    }
}

/** Rebuild an allowlisted shape; unrecognized fields never reach the host or persistence. */
export function validateLocalPlugin(value: unknown): LocalPluginManifest {
    if (!object(value)) throw new Error('The plugin manifest must be an object.')
    const id = requiredText(value.id, 80, 'id')
    if (!/^[a-z0-9][a-z0-9._-]{2,79}$/iu.test(id)) throw new Error('Invalid plugin id.')
    const name = requiredText(value.name, 80, 'name')
    const version = requiredText(value.version, 24, 'version')
    if (value.permissions !== undefined && !Array.isArray(value.permissions))
        throw new Error('Invalid permissions.')
    const permissions = (value.permissions ?? []) as unknown[]
    if (
        permissions.some(
            (permission) => !PLUGIN_PERMISSIONS.includes(permission as PluginPermission),
        )
    )
        throw new Error('The plugin requests an unsupported permission.')
    if (new Set(permissions).size !== permissions.length)
        throw new Error('Duplicate plugin permissions.')
    if (value.actions !== undefined && !Array.isArray(value.actions))
        throw new Error('Invalid actions.')
    const rawActions = (value.actions ?? []) as unknown[]
    if (rawActions.length > 12) throw new Error('A plugin can declare at most 12 actions.')
    const actions = rawActions.map((entry): PluginAction => {
        if (!object(entry)) throw new Error('Invalid plugin action.')
        const actionId = requiredText(entry.id, 40, 'action.id')
        if (!/^[a-z0-9_-]{1,40}$/iu.test(actionId)) throw new Error('Invalid action id.')
        const label = requiredText(entry.label, 48, 'action.label')
        const type = entry.type as PluginAction['type']
        if (!Object.prototype.hasOwnProperty.call(permissionForAction, type))
            throw new Error('Unsupported plugin action.')
        if (!permissions.includes(permissionForAction[type]))
            throw new Error(`Missing permission: ${permissionForAction[type]}.`)
        let actionValue: string | undefined
        if (type === 'toast') actionValue = requiredText(entry.value, 240, 'action.value')
        if (type === 'theme') {
            if (!PLUGIN_ACCENTS.includes(entry.value as PluginAccent))
                throw new Error('Invalid theme accent.')
            actionValue = entry.value as PluginAccent
        }
        if (type === 'navigate') {
            if (
                typeof entry.value !== 'string' ||
                !Object.prototype.hasOwnProperty.call(PLUGIN_TARGETS, entry.value)
            )
                throw new Error('Navigation target is not a built-in page.')
            actionValue = entry.value
        }
        return {
            id: actionId,
            label,
            type,
            ...(actionValue === undefined ? {} : { value: actionValue }),
        }
    })
    if (new Set(actions.map((action) => action.id)).size !== actions.length)
        throw new Error('Duplicate action ids.')
    let theme: PluginTheme | undefined
    if (value.theme !== undefined) {
        if (!object(value.theme) || !PLUGIN_ACCENTS.includes(value.theme.accent as PluginAccent))
            throw new Error('Invalid plugin theme.')
        if (!permissions.includes('ui.theme')) throw new Error('Missing permission: ui.theme.')
        const mode = value.theme.mode
        if (mode !== undefined && !['dark', 'light', 'oled'].includes(mode as string))
            throw new Error('Invalid theme mode.')
        theme = {
            accent: value.theme.accent as PluginAccent,
            ...(mode === undefined ? {} : { mode: mode as PluginTheme['mode'] }),
        }
    }
    if (value.panels !== undefined && !Array.isArray(value.panels))
        throw new Error('Invalid plugin panels.')
    const rawPanels = (value.panels ?? []) as unknown[]
    if (rawPanels.length > 4) throw new Error('A plugin can declare at most four panels.')
    const panels = rawPanels.map((entry, index): PluginPanel => {
        if (!object(entry) || entry.slot !== 'home.after')
            throw new Error('Unsupported plugin content slot.')
        const panelId = requiredText(entry.id, 40, 'panel.id')
        if (!/^[a-z0-9_-]{1,40}$/iu.test(panelId)) throw new Error('Invalid panel id.')
        const height = entry.height ?? 220
        if (typeof height !== 'number' || !Number.isInteger(height) || height < 120 || height > 600)
            throw new Error('Panel height must be between 120 and 600.')
        return { ...page(entry, `panels[${index}]`), id: panelId, slot: 'home.after', height }
    })
    if (new Set(panels.map((panel) => panel.id)).size !== panels.length)
        throw new Error('Duplicate panel ids.')
    const manifest: LocalPluginManifest = {
        id,
        name,
        version,
        permissions: permissions as PluginPermission[],
        actions,
        panels,
        ...(value.page === undefined ? {} : { page: page(value.page, 'page') }),
        ...(theme ? { theme } : {}),
        ...(value.description === undefined
            ? {}
            : { description: requiredText(value.description, 400, 'description') }),
    }
    if (!manifest.page && !manifest.theme && !manifest.actions.length && !manifest.panels.length)
        throw new Error('A plugin must provide a page, theme, action, or content panel.')
    if (new TextEncoder().encode(JSON.stringify(manifest)).length > MAX_MANIFEST_BYTES)
        throw new Error('The plugin manifest exceeds the size limit.')
    return manifest
}

export function parseLocalPlugin(text: string): LocalPluginManifest {
    if (new TextEncoder().encode(text).length > MAX_MANIFEST_BYTES)
        throw new Error('The plugin file exceeds the size limit.')
    return validateLocalPlugin(JSON.parse(text))
}

/** Invalid records are skipped, and a duplicate ID cannot shadow an earlier record. */
export function readLocalPlugins(text: string | null): InstalledLocalPlugin[] {
    if (!text) return []
    try {
        const value: unknown = JSON.parse(text)
        if (!Array.isArray(value) || value.length > 40) return []
        const plugins = new Map<string, InstalledLocalPlugin>()
        for (const entry of value) {
            try {
                if (!object(entry) || typeof entry.enabled !== 'boolean') continue
                const manifest = validateLocalPlugin(entry.manifest)
                if (!plugins.has(manifest.id))
                    plugins.set(manifest.id, { manifest, enabled: entry.enabled })
            } catch {
                // A corrupt or obsolete plugin cannot prevent the launcher from opening.
            }
        }
        return [...plugins.values()]
    } catch {
        return []
    }
}

export function writeLocalPlugins(plugins: InstalledLocalPlugin[]): string {
    if (plugins.length > 40) throw new Error('At most 40 local plugins can be installed.')
    return JSON.stringify(
        plugins.map((plugin) => ({
            manifest: validateLocalPlugin(plugin.manifest),
            enabled: plugin.enabled === true,
        })),
    )
}

export function pluginAccentToLauncher(accent: PluginAccent) {
    return { emerald: 'green', ocean: 'blue', violet: 'purple', amber: 'orange', rose: 'pink' }[
        accent
    ] as 'green' | 'blue' | 'purple' | 'orange' | 'pink'
}

export function isPluginRequest(value: unknown, session: string): value is PluginRequest {
    if (!object(value) || value.type !== 'mctrin:request' || value.session !== session) return false
    if (typeof value.requestId !== 'string' || !/^[a-z0-9_-]{1,64}$/iu.test(value.requestId))
        return false
    return (
        value.action === 'instances.list' ||
        (value.action === 'actions.run' &&
            typeof value.actionId === 'string' &&
            /^[a-z0-9_-]{1,40}$/iu.test(value.actionId))
    )
}

export async function runLocalPluginAction(
    plugin: InstalledLocalPlugin,
    actionId: string,
    host: PluginHost,
): Promise<boolean> {
    if (!plugin.enabled) throw new Error('The plugin is disabled.')
    const action = plugin.manifest.actions.find((candidate) => candidate.id === actionId)
    if (!action) throw new Error('Unknown plugin action.')
    if (!plugin.manifest.permissions.includes(permissionForAction[action.type]))
        throw new Error('The plugin lacks permission for this action.')
    if (action.type === 'toast') host.toast(action.value!)
    if (action.type === 'theme') await host.theme({ accent: action.value as PluginAccent })
    if (action.type === 'navigate') await host.navigate(action.value as PluginTarget)
    if (action.type === 'launchDefault') return await host.launchDefault(plugin.manifest.name)
    return true
}

/** The host binds messages to a particular iframe WindowProxy before calling this. */
export async function dispatchLocalPluginRequest(
    plugin: InstalledLocalPlugin,
    request: PluginRequest,
    host: PluginHost,
): Promise<PluginResponse> {
    const respond = (ok: boolean, value: unknown): PluginResponse => ({
        type: 'mctrin:response',
        requestId: request.requestId,
        session: request.session,
        ok,
        value,
    })
    try {
        if (!plugin.enabled) throw new Error('The plugin is disabled.')
        if (request.action === 'instances.list') {
            if (!plugin.manifest.permissions.includes('instances.read'))
                throw new Error('Missing permission: instances.read.')
            const items = await host.instances()
            // Redact by reconstruction even if the platform helper returns extra properties.
            return respond(
                true,
                items.slice(0, 500).map((item) => ({
                    id: item.id,
                    name: item.name,
                    gameVersion: item.gameVersion,
                    loader: item.loader,
                    selected: item.selected,
                })),
            )
        }
        const result = await runLocalPluginAction(plugin, request.actionId!, host)
        return respond(result, result ? true : 'The action was declined or failed.')
    } catch (error) {
        return respond(false, error instanceof Error ? error.message : 'The plugin request failed.')
    }
}

export function buildLocalPluginDocument(
    content: PluginPage,
    session: string,
    bootstrapUrl: string,
): string {
    if (!/^[a-z0-9_-]{16,80}$/iu.test(session)) throw new Error('Invalid sandbox session.')
    const bootstrap = new URL(bootstrapUrl)
    if (
        !['https:', 'http:', 'tauri:'].includes(bootstrap.protocol) ||
        bootstrap.pathname !== '/mctrin-plugin-bootstrap.js' ||
        bootstrap.search ||
        bootstrap.hash ||
        bootstrap.username ||
        bootstrap.password
    )
        throw new Error('Invalid sandbox bootstrap URL.')
    const source = bootstrap.href.replaceAll('"', '&quot;')
    const policy = `default-src 'none'; script-src ${source} 'unsafe-eval'; script-src-attr 'none'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; media-src 'none'; font-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'`
    const script = JSON.stringify(content.script ?? '')
        .replaceAll('<', '\\u003c')
        .replaceAll('>', '\\u003e')
        .replaceAll('&', '\\u0026')
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${policy}"><meta name="mctrin-session" content="${session}"><meta name="referrer" content="no-referrer"><style>html{color-scheme:dark light}body{margin:16px;font:14px system-ui,sans-serif;line-height:1.5;overflow-wrap:anywhere}button,input,textarea,select{font:inherit}a{color:inherit}</style></head><body>${content.html}<script id="mctrin-plugin-script" type="application/json">${script}</script><script src="${source}"></script></body></html>`
}
