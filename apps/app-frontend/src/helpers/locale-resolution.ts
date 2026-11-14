/**
 * Canonical locale resolution for the desktop frontend.
 *
 * Every path that turns a requested locale into an activated message catalogue
 * goes through this module: `setAppLocale`, `applyLocalePreference` and
 * `resolveInitialLocale`. The settings picker, the onboarding picker and the
 * OS-language path therefore cannot disagree about what a tag means.
 *
 * Resolution chain, in order:
 *
 * 1. Exact catalogue match, ignoring case and `_`/`-` differences
 *    (`zh_CN`, `ZH-cn` -> `zh-CN`).
 * 2. Chinese script/region mapping: `zh-Hant`, `zh-TW`, `zh-HK` and `zh-MO`
 *    select `zh-TW`; `zh-Hans` and every other `zh` tag select `zh-CN`
 *    (bare `zh` defaults to Simplified per BCP 47).
 * 3. Language-only match (`de` -> the first shipped `de-*` catalogue).
 * 4. `en-US`, the only catalogue that is always shipped.
 *
 * A tag that matches nothing becomes `en-US`. It never falls through to
 * "whichever catalogue happens to be first" and never to a third language,
 * because silently activating an arbitrary catalogue is what produced
 * mixed-language chrome.
 */
export const DEFAULT_LOCALE = 'en-US'

/** Lower-cases a tag and unifies separators so `zh_CN` and `ZH-cn` compare equal. */
export function normalizeLocaleTag(tag: string): string {
    return tag.trim().replace(/_/g, '-').toLowerCase()
}

function findExact(requested: string, candidates: readonly string[]): string | undefined {
    const wanted = normalizeLocaleTag(requested)
    if (!wanted) return undefined
    return candidates.find((locale) => normalizeLocaleTag(locale) === wanted)
}

function findLanguage(language: string, candidates: readonly string[]): string | undefined {
    if (!language) return undefined
    return candidates.find((locale) => normalizeLocaleTag(locale).split('-')[0] === language)
}

function matchChinese(requested: string, candidates: readonly string[]): string | undefined {
    const [language, ...subtags] = normalizeLocaleTag(requested).split('-')
    if (language !== 'zh') return undefined

    const traditional =
        subtags.includes('hant') || subtags.some((subtag) => ['tw', 'hk', 'mo'].includes(subtag))
    const target = traditional ? 'zh-TW' : 'zh-CN'
    return findExact(target, candidates) ?? findLanguage('zh', candidates)
}

/**
 * Returns the catalogue that should back `requested`, or `undefined` when
 * nothing matches. Callers that must activate a catalogue use `resolveLocale`.
 */
export function matchLocale(
    requested: string | null | undefined,
    available: readonly string[],
): string | undefined {
    if (!requested) return undefined

    return (
        findExact(requested, available) ??
        matchChinese(requested, available) ??
        findLanguage(normalizeLocaleTag(requested).split('-')[0], available)
    )
}

/**
 * Resolves a single requested locale to a shipped catalogue, falling back to
 * `en-US` when the tag is unknown or unsupported.
 */
export function resolveLocale(
    requested: string | null | undefined,
    available: readonly string[],
): string {
    return matchLocale(requested, available) ?? DEFAULT_LOCALE
}

/**
 * Resolves an ordered preference list (for example `navigator.languages`) to the
 * first preference that has a catalogue. An empty or fully unsupported list
 * resolves to `en-US`.
 */
export function resolvePreferredLocale(
    preferred: readonly string[],
    available: readonly string[],
): string {
    for (const candidate of preferred) {
        const match = matchLocale(candidate, available)
        if (match) return match
    }
    return DEFAULT_LOCALE
}
