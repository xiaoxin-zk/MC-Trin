import { find_filtered_jres, get_java_default_versions, set_java_default_version } from './jre.js'

type JavaScan = () => Promise<unknown>
type JavaInstallation = {
    path?: unknown
    parsed_version?: unknown
}

const SUPPORTED_JAVA_VERSIONS = [25, 21, 17, 8]

async function syncMissingJavaDefaults(result: unknown): Promise<void> {
    if (!Array.isArray(result)) return

    const installations = result.filter((value): value is JavaInstallation =>
        Boolean(value && typeof value === 'object'),
    )
    if (!installations.length) return

    const defaults = await get_java_default_versions()
    const configured = new Set(
        defaults
            .map((java) => java?.parsed_version)
            .filter((version): version is number => typeof version === 'number'),
    )

    for (const majorVersion of SUPPORTED_JAVA_VERSIONS) {
        if (configured.has(majorVersion)) continue
        const candidate = installations.find(
            (java) =>
                java.parsed_version === majorVersion &&
                typeof java.path === 'string' &&
                java.path.trim().length > 0,
        )
        if (!candidate || typeof candidate.path !== 'string') continue

        try {
            await set_java_default_version(majorVersion, candidate.path)
            configured.add(majorVersion)
        } catch (error) {
            console.warn(`Failed to set Java ${majorVersion} default on startup`, error)
        }
    }
}

export function createStartupJavaScan(
    scan: JavaScan = () => find_filtered_jres(null, false, true, false),
    onError: (error: unknown) => void = (error) => {
        console.warn('Failed to scan installed Java runtimes on startup', error)
    },
): () => void {
    let started = false

    return () => {
        if (started) return
        started = true
        void scan()
            .then((result) => syncMissingJavaDefaults(result))
            .catch(onError)
    }
}

export const startStartupJavaScan = createStartupJavaScan()
