/**
 * How long the renderer waits for the user to answer an approval dialog.
 *
 * Mirrors `APPROVAL_TTL` in `packages/app-lib/src/api/logs/logagent/mod.rs`.
 * A grant the backend minted stops being redeemable after 120 s, so waiting
 * longer can only produce a token the Rust gate rejects. The same budget
 * therefore bounds the renderer promise, which would otherwise stay pending
 * forever when the dialog never appears.
 */
export const APPROVAL_TIMEOUT_MS = 120_000

/**
 * Hard ceiling for one assistant turn.
 *
 * The backend expires a run 10 minutes after it was prepared (`SESSION_TTL`),
 * so a shorter renderer deadline would abort work the launcher can still
 * finish. One minute of grace lets the backend's own "time budget exceeded"
 * error win while still ending a turn whose promise never settles.
 */
export const TURN_DEADLINE_MS = 11 * 60_000

/** Shown when a turn is stopped by `TURN_DEADLINE_MS`. */
export const TURN_DEADLINE_MESSAGE =
    'The assistant stopped responding and this turn was cancelled. Try again.'

/**
 * Raised when a turn exceeds its deadline. A distinct type lets the caller
 * report the timeout even though it aborts the run's signal while settling.
 */
export class TurnDeadlineError extends Error {
    constructor(message: string) {
        super(message)
        this.name = 'TurnDeadlineError'
    }
}

/**
 * Races `operation` against a timer. The timer is always cleared once the
 * operation settles, so a completed turn never keeps an idle callback alive.
 * `onTimeout` runs before the rejection so the caller can abort the work the
 * deadline just gave up on.
 */
export function withTurnDeadline<T>(
    operation: Promise<T>,
    timeoutMs: number,
    message: string,
    onTimeout?: () => void,
): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined
    return Promise.race([
        operation,
        new Promise<never>((_, reject) => {
            timer = setTimeout(() => {
                onTimeout?.()
                reject(new TurnDeadlineError(message))
            }, timeoutMs)
        }),
    ]).finally(() => {
        if (timer !== undefined) clearTimeout(timer)
    })
}
