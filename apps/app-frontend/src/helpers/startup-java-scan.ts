import { find_filtered_jres } from './jre.js'

type JavaScan = () => Promise<unknown>

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
        void scan().catch(onError)
    }
}

export const startStartupJavaScan = createStartupJavaScan()
