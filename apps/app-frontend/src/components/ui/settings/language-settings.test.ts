import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

const languageSettings = readFileSync(new URL('./LanguageSettings.vue', import.meta.url), 'utf8')
const onboardingWelcome = readFileSync(
    new URL('../onboarding/OnboardingWelcome.vue', import.meta.url),
    'utf8',
)
const settingsPage = readFileSync(new URL('../../../pages/Settings.vue', import.meta.url), 'utf8')
const i18nConfig = readFileSync(new URL('../../../i18n.config.ts', import.meta.url), 'utf8')

const IPC_LATENCY = 3

function delay(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * The settings page reduced to what the defect needs: the stored row, the locale
 * the page renders, and the language shown by the mounted section.
 *
 * `Settings.vue` keys its content scroll container on `activeLocale`, so
 * changing `rendered` destroys the language section and mounts a replacement.
 * The replacement paints the row its own `settings_get` SELECT returns, which is
 * what makes the order of the write and the locale flip decide what it shows.
 */
interface FakePage {
    row: string
    rendered: string
    picker: string
    events: string[]
}

function createFakePage(locale: string): FakePage {
    return { row: locale, rendered: locale, picker: locale, events: [] }
}

/** The SELECT a freshly mounted language section issues before it can paint. */
function readRow(page: FakePage, paint: (locale: string) => void) {
    page.events.push('get:issued')
    // The SELECT sees the row as it is when the query runs, not when it settles.
    const snapshot = page.row
    setTimeout(() => {
        page.events.push('get:returned')
        paint(snapshot)
    }, IPC_LATENCY)
}

/** Reproduces the shipped order: activate first, then write. */
async function switchThenSave(page: FakePage, locale: string) {
    page.events.push('activate')
    await delay(IPC_LATENCY)
    page.rendered = locale
    // The locale change remounts the section before the write is issued.
    readRow(page, (row) => {
        page.picker = row
    })
    page.events.push('patch:issued')
    await delay(IPC_LATENCY)
    page.row = locale
    page.events.push('patch:applied')
}

test('the language picker stores the locale that actually became active', () => {
    // Storing the requested tag instead let the database and the picker claim a
    // language the UI was not rendering, because the resolver may fall back (an
    // unknown tag resolves to en-US) and the loader drops an overtaken
    // activation. The write therefore stores the code that actually became
    // active, and the onboarding picker keeps doing so directly.
    assert.match(languageSettings, /prepare: prepareAppLocale/)
    assert.match(languageSettings, /activate: setAppLocale/)
    assert.match(i18nConfig, /export async function prepareAppLocale\(requested: string\)/)
    assert.match(onboardingWelcome, /const appliedLocale = await setAppLocale\(concreteLocale\)/)
    assert.match(onboardingWelcome, /await update\(\{ locale: appliedLocale \}\)/)
})

test('opening the language list does not start a competing locale write', () => {
    // The open handler used to call `persistLocale`, so opening the dropdown and
    // then picking a language left two runs racing; whichever finished last won.
    const openHandler = languageSettings.match(/@open="([^"]+)"/)?.[1]
    assert.equal(openHandler, 'beginManualSelection')
    assert.match(
        languageSettings,
        /function beginManualSelection\(\)[\s\S]*?setFollowSystemLocale\(false\)/,
    )
    assert.doesNotMatch(
        languageSettings,
        /function beginManualSelection\(\)[\s\S]{0,300}?await persistLocale/,
    )
})

test('the settings page remounts its content when the language changes', () => {
    // The reported defect: switching language on the settings page saved the
    // value ("Saved" appeared) but left the page in the old language until the
    // user navigated away, because the mounted category body never re-resolved.
    // This remount is what makes every other category show copy in the new
    // language immediately, so a fix has to order the save around it instead of
    // removing it.
    assert.match(settingsPage, /locale: activeLocale \} = useVIntl\(\)/)
    assert.match(settingsPage, /:key="activeLocale"[\s\S]{0,400}?class="settings-content-scroll/)
})

test('one resolver serves every locale entry point', () => {
    // P0-1 regressed when the picker and the startup path each decided what a
    // tag meant. `setAppLocale`, `applyLocalePreference` and `resolveInitialLocale`
    // must all delegate to the shared chain in `helpers/locale-resolution`.
    assert.match(i18nConfig, /resolveLocale\(requested, availableLocales\)/)
    assert.match(i18nConfig, /matchLocale\(preferredLocale, availableLocales\)/)
    assert.doesNotMatch(i18nConfig, /availableLocales\.includes\(locale\) \? locale : 'en-US'/)
    assert.doesNotMatch(i18nConfig, /traditionalChinese \? 'zh-TW' : 'zh-CN'/)
})

test('only codes present in LOCALES and on disk are loadable', () => {
    // A directory without a `LOCALES` entry (ar-SA) loads an empty bundle, and
    // vue-i18n then falls back per message tree, which mixes two languages.
    assert.match(
        i18nConfig,
        /\.filter\(\(locale\) => LOCALES\.some\(\(definition\) => definition\.code === locale\)\)/,
    )
    assert.match(i18nConfig, /Locale bundle for \$\{locale\} is empty/)
})

test('the language switch stores the preference before it remounts the section that owns the save', async () => {
    // The reported defect. `persistLocale` flipped the locale first (which tore
    // down the section) and only wrote afterwards, so the replacement's
    // `settings_get` SELECT ran before `settings_patch` reached the backend. The
    // picker then painted the row the switch was replacing while the page
    // rendered the new language.
    //
    // First show the shipped order actually produces that state, so the test
    // states the defect instead of assuming it.
    const reproduced = createFakePage('en-US')
    await switchThenSave(reproduced, 'zh-CN')
    await delay(IPC_LATENCY * 4)
    assert.equal(
        reproduced.picker,
        'en-US',
        'the shipped switch-then-save order must reproduce the stale picker, otherwise this test no longer guards the reported defect',
    )
    assert.equal(reproduced.rendered, 'zh-CN', 'the page renders the new language')
    assert.equal(reproduced.row, 'zh-CN', 'the database holds the new language')

    // The fix has to exist as an ordered save step the component routes through.
    const { createLocalePersistence } = await import('./locale-persistence.ts').catch(() => ({
        createLocalePersistence: undefined,
    }))
    assert.ok(
        createLocalePersistence,
        'the save has no ordering step, so the locale is switched (and the section remounted) before the preference is stored',
    )

    // And the section must no longer flip the locale itself while saving: doing
    // so is what destroyed the instance before its own write was issued.
    const saveRoutine = languageSettings.match(/async function persistLocale\([\s\S]*?\n\}/)?.[0]
    assert.ok(saveRoutine, 'the save routine still exists')
    assert.doesNotMatch(
        saveRoutine,
        /await setAppLocale\(/,
        'persistLocale must not switch the locale itself before the write, because the switch remounts the section that owns the save',
    )

    const page = createFakePage('en-US')
    readRow(page, (locale) => {
        page.picker = locale
    })

    const persistence = createLocalePersistence({
        prepare: async (requested) => requested,
        write: async (locale) => {
            page.events.push('patch:issued')
            await delay(IPC_LATENCY)
            page.row = locale
            page.events.push('patch:applied')
        },
        activate: async (locale) => {
            page.events.push('activate')
            // The locale change remounts the section, exactly like the page does.
            await delay(IPC_LATENCY)
            page.rendered = locale
            readRow(page, (row) => {
                page.picker = row
            })
            return locale
        },
    })

    const outcome = await persistence.persist('zh-CN')
    assert.deepEqual(outcome, { status: 'saved', locale: 'zh-CN' })

    // Settle every read the remount issued.
    await delay(IPC_LATENCY * 6)

    assert.equal(page.row, 'zh-CN', 'the database kept the chosen language')
    assert.equal(page.rendered, 'zh-CN', 'the page renders the chosen language')
    assert.equal(
        page.picker,
        'zh-CN',
        'the picker of the remounted section shows the language that was saved, not the one it replaced',
    )

    const writeApplied = page.events.indexOf('patch:applied')
    const activated = page.events.indexOf('activate')
    assert.ok(writeApplied >= 0, 'the preference was written')
    assert.ok(
        writeApplied < activated,
        `the preference must be stored before the locale changes, but the order was ${page.events.join(' -> ')}`,
    )
})

test('a failed save fails on the section that is still mounted', async () => {
    // The failure half of the same defect: because the locale was flipped before
    // the write, the catch block restored the old language and set `saveStatus`
    // on a destroyed instance, so no "Could not save" and no retry ever showed.
    const { createLocalePersistence } = await import('./locale-persistence.ts').catch(() => ({
        createLocalePersistence: undefined,
    }))
    assert.ok(createLocalePersistence, 'the save has no ordering step')

    const page = createFakePage('en-US')
    const persistence = createLocalePersistence({
        prepare: async (requested) => requested,
        write: async () => {
            await delay(IPC_LATENCY)
            throw new Error('settings_patch failed')
        },
        activate: async (locale) => {
            page.events.push('activate')
            page.rendered = locale
            return locale
        },
    })

    const outcome = await persistence.persist('zh-CN')

    assert.equal(outcome.status, 'failed', 'the caller learns that the save failed')
    assert.equal(page.rendered, 'en-US', 'the page still renders the stored language')
    assert.equal(page.row, 'en-US', 'the stored preference is unchanged')
    assert.deepEqual(
        page.events.filter((event) => event === 'activate'),
        [],
        'a failed save never changes the locale, so the section that reports it stays mounted',
    )
})

test('a second pick cannot leave the database behind the rendered language', async () => {
    // Two picks overlap whenever the second starts before the first write
    // returns. If the older write reaches the backend last, the database keeps
    // the earlier choice while the page renders the later one.
    const { createLocalePersistence } = await import('./locale-persistence.ts').catch(() => ({
        createLocalePersistence: undefined,
    }))
    assert.ok(createLocalePersistence, 'the save has no ordering step')

    const page = createFakePage('en-US')
    const persistence = createLocalePersistence({
        prepare: async (requested) => requested,
        write: (locale) =>
            new Promise<void>((resolve) => {
                // The earlier pick is slower, so an unqueued pair would invert.
                setTimeout(
                    () => {
                        page.row = locale
                        resolve()
                    },
                    locale === 'de-DE' ? IPC_LATENCY * 4 : IPC_LATENCY,
                )
            }),
        activate: async (locale) => {
            page.rendered = locale
            return locale
        },
    })

    const [first, second] = await Promise.all([
        persistence.persist('de-DE'),
        persistence.persist('zh-CN'),
    ])
    await delay(IPC_LATENCY * 8)

    assert.equal(first.status, 'superseded', 'the overtaken pick stands down')
    assert.equal(second.status, 'saved', 'the newest pick commits')
    assert.equal(page.rendered, 'zh-CN', 'the page renders the newest pick')
    assert.equal(
        page.row,
        'zh-CN',
        'the database keeps the newest pick, not the slower earlier one',
    )
})

test('the ordering guard outlives the section instance that started the run', () => {
    // The section is destroyed by the very switch it reports on, so a counter
    // held in component state could not stop a run that outlived its instance.
    assert.match(
        languageSettings,
        /import \{ createLocalePersistence \} from '\.\/locale-persistence'/,
    )
    assert.match(languageSettings, /const outcome = await localePersistence\.persist\(/)
    assert.match(languageSettings, /if \(outcome\.status === 'superseded'\) return/)
    assert.doesNotMatch(languageSettings, /let localeRun = 0/)
    assert.doesNotMatch(languageSettings, /const run = \+\+localeRun/)
})
