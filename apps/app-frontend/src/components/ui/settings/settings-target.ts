/**
 * Deep link to one control inside a settings category.
 *
 * The settings page renders a single category at a time and scrolls to a
 * control through `pendingSearchTarget` (that is what a search result does).
 * Callers outside the page — an empty state that points at the setting which
 * fixes it — need the same behaviour, so the destination is expressed once
 * here and read back by `Settings.vue` instead of both sides inventing a
 * shape of their own.
 */

const SETTINGS_TARGET_QUERY = 'target'
const SAFE_SEGMENT = /^[A-Za-z0-9\-_]+$/

export interface SettingsTargetRoute {
    path: '/settings'
    hash: string
    query?: Record<string, string>
}

/**
 * Builds the route for `settings-target-<target>` inside `tab`.
 *
 * `target` is the short id used by the settings search index (`log-share`),
 * not a DOM id, so a stale or hand-written link can only fail to resolve; it
 * can never address an arbitrary element.
 */
export function settingsTargetRoute(tab: string, target?: string): SettingsTargetRoute {
    const route: SettingsTargetRoute = {
        path: '/settings',
        hash: `#${SAFE_SEGMENT.test(tab) ? tab : 'interface'}`,
    }
    if (target && SAFE_SEGMENT.test(target)) route.query = { [SETTINGS_TARGET_QUERY]: target }
    return route
}

/** Reads the anchor back out of a settings route, or `null` when there is none. */
export function resolveSettingsTarget(
    hash: string,
    query: Record<string, unknown>,
): { categoryId: string; targetId: string } | null {
    const raw = query[SETTINGS_TARGET_QUERY]
    const target = typeof raw === 'string' ? raw : ''
    if (!target || !SAFE_SEGMENT.test(target)) return null
    const categoryId = hash.replace(/^#/, '')
    if (!SAFE_SEGMENT.test(categoryId)) return null
    return { categoryId, targetId: `settings-target-${target}` }
}
