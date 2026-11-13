const LEGACY_KEY_PREFIX = 'axolotl'
const NEW_KEY_PREFIX = 'mctrin'

/**
 * Renames legacy `axolotl*` localStorage keys to their `mctrin*` equivalents,
 * once per session. For every legacy key the value is copied to the new key
 * when the new key does not exist yet, after which the legacy key is removed.
 * Keys whose new name already exists are dropped without overwriting newer
 * data. Runs before any feature code reads storage: main.js calls this as its
 * first statement, and every storage read happens lazily afterwards.
 */
export function migrateLegacyStorageKeys(): void {
    try {
        const legacyKeys: string[] = []
        for (let i = 0; i < localStorage.length; i++) {
            const key = localStorage.key(i)
            if (key?.startsWith(LEGACY_KEY_PREFIX)) legacyKeys.push(key)
        }
        for (const legacyKey of legacyKeys) {
            const newKey = `${NEW_KEY_PREFIX}${legacyKey.slice(LEGACY_KEY_PREFIX.length)}`
            try {
                if (localStorage.getItem(newKey) === null) {
                    const value = localStorage.getItem(legacyKey)
                    if (value !== null) localStorage.setItem(newKey, value)
                }
                localStorage.removeItem(legacyKey)
            } catch {
                // Skip the unreadable entry; the remaining keys still migrate.
            }
        }
    } catch {
        // localStorage may be unavailable; every reader already handles that.
    }
}
