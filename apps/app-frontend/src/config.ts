const trimTrailingSlash = (url: string) => url.replace(/\/$/, '')

export const MCTrinBrandConfig = Object.freeze({
    productName: 'MC-Trin',
    shortProductName: 'MC-Trin',
    website: '',
    repositoryUrl: '',
    supportUrl: 'https://github.com/xiaoxin-zk/MC-Trin/issues',
    qqGroupNumber: '',
    qqChannelUrl: '',
    sponsorUrl: '',
    bundleIdentifier: 'com.mctrin.launcher',
    deepLinkScheme: 'mctrin',
    userAgent: (version: string, os: string) => `MC-Trin/${version} (${os})`,
    capabilities: Object.freeze({
        publicModrinthApi: true,
        privateModrinthServices: false,
        ghsTelemetry: false,
    }),
})

// Keep the old export for modules that still carry upstream compatibility code.
// User-facing surfaces should import MCTrinBrandConfig directly.
export const AxolotlBrandConfig = MCTrinBrandConfig

const siteUrl = trimTrailingSlash(import.meta.env.MODRINTH_URL || 'https://modrinth.com')
const officialLabrinthBaseUrl = trimTrailingSlash(
    import.meta.env.MODRINTH_API_BASE_URL || 'https://api.modrinth.com',
)
type DownloadSourceMode = 'auto' | 'official_only' | 'mirror_preferred' | 'official_preferred'

// The Modrinth API always uses the official source; Modrinth download mirror
// routing is handled by the Rust download layer.
export function setModrinthSourceMode(_sourceMode: DownloadSourceMode) {}

export function setModrinthMirrorEnabled(_enabled: boolean) {}

export function getOfficialLabrinthBaseUrl() {
    return officialLabrinthBaseUrl
}

export function getLabrinthBaseUrl() {
    return officialLabrinthBaseUrl
}

export const config = {
    siteUrl,
    labrinthBaseUrl: getLabrinthBaseUrl,
}
