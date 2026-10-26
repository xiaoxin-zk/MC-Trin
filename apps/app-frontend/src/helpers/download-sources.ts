export type CustomDownloadSourceKind = 'minecraft' | 'modrinth' | 'curseforge'

export type CustomDownloadSource = {
    id: string
    name: string
    kind: CustomDownloadSourceKind
    base_url: string
    enabled: boolean
    priority: number
}

export function normalizeDownloadSourceUrl(input: string): string {
    const value = input.trim()
    if (!value || value.length > 512) {
        throw new Error('Enter an HTTPS base URL up to 512 characters long.')
    }

    let url: URL
    try {
        url = new URL(value)
    } catch {
        throw new Error('Enter a valid HTTPS base URL.')
    }

    if (
        url.protocol !== 'https:' ||
        !url.hostname ||
        url.username ||
        url.password ||
        url.search ||
        url.hash
    ) {
        throw new Error('Use HTTPS without credentials, query parameters, or fragments.')
    }

    return `${url.origin}${url.pathname.replace(/\/+$/, '')}`
}

export function orderDownloadSources(sources: CustomDownloadSource[]): CustomDownloadSource[] {
    return [...sources]
        .sort((left, right) => left.priority - right.priority)
        .map((source, priority) => ({ ...source, priority }))
}
