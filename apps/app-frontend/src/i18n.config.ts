import {
    buildLocaleMessages,
    createMessageCompiler,
    type CrowdinMessages,
    LOCALES,
    uiEnglishMessages,
    uiLocaleModules,
} from '@modrinth/ui'
import { createI18n } from 'vue-i18n'

import { createLocaleLoader } from '@/helpers/locale-loader'
import { DEFAULT_LOCALE, matchLocale, resolveLocale } from '@/helpers/locale-resolution'
import englishAppMessages from '@/locales/en-US/index.json'

const localeModules = import.meta.glob<{ default: CrowdinMessages }>('./locales/*/index.json')

/**
 * Catalogues that can actually be activated. A locale directory alone is not
 * enough: `buildLocaleMessages` drops codes that are missing from `LOCALES`, so a
 * directory without a `LOCALES` entry (currently `ar-SA`) loads an empty bundle
 * while `locale.value` still points at it. Every string then falls back per
 * message tree, which renders two languages at once. Intersecting both sources
 * keeps the selectable list and the loadable list identical.
 */
const availableLocales = Object.keys(localeModules)
    .map((path) => path.split('/').at(-2)!)
    .filter((locale) => LOCALES.some((definition) => definition.code === locale))

const STARTUP_LOCALE_KEY = 'mctrin.startup-locale'

const i18n = createI18n({
    legacy: false,
    locale: DEFAULT_LOCALE,
    fallbackLocale: DEFAULT_LOCALE,
    messageCompiler: createMessageCompiler(),
    missingWarn: false,
    fallbackWarn: false,
    messages: buildLocaleMessages(
        { './locales/en-US/index.json': { default: englishAppMessages } },
        { './locales/en-US/index.json': { default: uiEnglishMessages } },
    ),
})

const localeLoader = createLocaleLoader(
    async (locale) => {
        const appPath = `./locales/${locale}/index.json`
        const [appMessages, uiMessages] = await Promise.all([
            localeModules[appPath](),
            uiLocaleModules[appPath](),
        ])
        const messages = buildLocaleMessages({ [appPath]: appMessages }, { [appPath]: uiMessages })[
            locale
        ]
        if (!messages || Object.keys(messages).length === 0) {
            // Activating an empty bundle makes vue-i18n fall back per message tree
            // and the UI renders in two languages at once.
            throw new Error(`Locale bundle for ${locale} is empty`)
        }
        i18n.global.setLocaleMessage(locale, messages)
    },
    (locale) => {
        i18n.global.locale.value = locale
    },
    [DEFAULT_LOCALE],
)

/**
 * Activates `requested` through the shared resolution chain and returns the
 * locale that ends up active.
 *
 * Every caller goes through here, so a tag can only ever become an exact
 * catalogue, one of the two Chinese script mappings, a language-only match, or
 * `en-US`. No caller picks a catalogue on its own, which is what previously let
 * the language picker, the stored preference and the OS-language path disagree.
 *
 * The loader drops an activation that a newer selection overtook. Reporting the
 * *requested* code in that case made a caller believe the switch happened while
 * the UI still rendered the previous language, so the picker and the database
 * showed the new language next to stale chrome. The active locale is therefore
 * read back instead of assumed.
 */
async function activateLocale(requested: string | null | undefined): Promise<string> {
    const target = resolveLocale(requested, availableLocales)
    await localeLoader.select(target)
    return getActiveLocale()
}

/**
 * Activates a locale selected by the user. Returns the locale that is actually
 * active afterwards so callers store the truth instead of the requested tag.
 */
export async function setAppLocale(locale: string): Promise<string> {
    return await activateLocale(locale)
}

/** Resolves and activates a stored or system preference, returning the concrete locale. */
export async function applyLocalePreference(
    storedLocale: string | null | undefined,
): Promise<string> {
    const locale = await activateLocale(
        isFollowingSystemLocale(storedLocale) ? getSystemResolvedLocale() : storedLocale,
    )
    try {
        localStorage.setItem(STARTUP_LOCALE_KEY, storedLocale ?? SYSTEM_LOCALE_VALUE)
    } catch {
        // Caching the preference only avoids loading an unused system dictionary next time.
    }
    return locale
}

export async function initializeLocale(): Promise<void> {
    let storedLocale: string | null = null
    try {
        storedLocale = localStorage.getItem(STARTUP_LOCALE_KEY)
    } catch {
        // The database preference remains authoritative when storage is unavailable.
    }
    await applyLocalePreference(storedLocale)
}

/**
 * Resolves an ordered preference list (for example `navigator.languages`) to a
 * shipped catalogue. Every entry goes through the same chain as every other
 * locale request, so `zh-Hant`/`zh-Hans` map onto `zh-TW`/`zh-CN` and no tag ever
 * resolves to an arbitrary catalogue.
 */
export function resolveInitialLocale(preferredLocales: readonly string[]): string {
    for (const preferredLocale of preferredLocales) {
        const match = matchLocale(preferredLocale, availableLocales)
        if (match) return match
    }
    return DEFAULT_LOCALE
}

export const SYSTEM_LOCALE_VALUE = 'system'
const FOLLOW_SYSTEM_LOCALE_KEY = 'mctrin.follow-system-locale'

function preferredNavigatorLocales(): readonly string[] {
    return navigator.languages?.length ? navigator.languages : [navigator.language ?? 'en-US']
}

export function getSystemResolvedLocale(): string {
    return resolveInitialLocale(preferredNavigatorLocales())
}

export function isFollowingSystemLocale(storedLocale: string | null | undefined): boolean {
    if (!storedLocale || storedLocale === SYSTEM_LOCALE_VALUE) return true
    try {
        return localStorage.getItem(FOLLOW_SYSTEM_LOCALE_KEY) === '1'
    } catch {
        return false
    }
}

export function setFollowSystemLocale(follow: boolean) {
    try {
        if (follow) localStorage.setItem(FOLLOW_SYSTEM_LOCALE_KEY, '1')
        else localStorage.removeItem(FOLLOW_SYSTEM_LOCALE_KEY)
    } catch {
        // localStorage may be unavailable; system-follow then relies on empty/'system' locale only.
    }
}

/** The locale whose bundle is loaded and active right now. */
export function getActiveLocale(): string {
    return i18n.global.locale.value
}

// English is available for the first synchronous render. Other dictionaries load
// alongside backend initialization; setupApp applies the authoritative DB preference.
export default i18n
