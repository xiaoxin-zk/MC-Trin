/**
 * Shared budget for the Mods page's content operations.
 *
 * File mutations, dependency installs and version updates all talk to the
 * launcher over the same channel; without a bound a stuck backend call leaves
 * the page in a permanent busy state with no way back.
 */
export const CONTENT_OPERATION_TIMEOUT_MS = 30_000

/**
 * Runs `operation`, rejecting with `timeoutMessage` if it takes longer than the
 * content-operation budget. The timer is always cleared so a settled operation
 * never keeps an idle callback alive.
 */
export async function withContentOperationTimeout<T>(
    operation: Promise<T>,
    timeoutMessage: string,
    timeoutMs: number = CONTENT_OPERATION_TIMEOUT_MS,
): Promise<T> {
    let timeoutId: ReturnType<typeof setTimeout> | undefined
    try {
        return await Promise.race([
            operation,
            new Promise<T>((_, reject) => {
                timeoutId = setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs)
            }),
        ])
    } finally {
        if (timeoutId) clearTimeout(timeoutId)
    }
}
