/**
 * Diagnostics that belong in a log, never in a dialog: Rust's `Location:`
 * trailer names internal source paths, and a backtrace footer tells the user to
 * set an environment variable they cannot act on.
 */
const INTERNAL_DIAGNOSTIC_PATTERNS = [
    /\s*Location:\s*\S+/u,
    /\s*Stack backtrace:[\s\S]*/u,
    /\s*note: run with `RUST_BACKTRACE[\s\S]*/u,
]

/**
 * Turns a backend error into the one sentence a user can act on.
 *
 * AI failures are string errors from the Rust side and can carry an eyre span
 * trace; the launcher also keeps error text in notification history on disk, so
 * internal paths and stack noise are stripped before anything is displayed.
 */
export function sanitizeAiErrorText(text: string): string {
    let cleaned = text
    for (const pattern of INTERNAL_DIAGNOSTIC_PATTERNS) {
        cleaned = cleaned.replace(pattern, '')
    }
    cleaned = cleaned.trim()
    const firstLine = cleaned.split(/\n/u)[0]?.trim() ?? ''
    return firstLine || cleaned
}
