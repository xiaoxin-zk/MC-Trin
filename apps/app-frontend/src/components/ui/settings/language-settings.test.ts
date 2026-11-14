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

test('the language picker stores the locale that actually became active', () => {
    // Storing the requested tag instead let the database and the picker claim a
    // language the UI was not rendering, because `setAppLocale` may fall back
    // (an unknown tag resolves to en-US) or drop an overtaken activation.
    assert.match(languageSettings, /const appliedLocale = await setAppLocale\(concreteLocale\)/)
    assert.match(languageSettings, /await update\(\{ locale: appliedLocale \}\)/)
    assert.doesNotMatch(languageSettings, /await update\(\{ locale: concreteLocale \}\)/)
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

test('only the newest locale write may commit', () => {
    assert.match(languageSettings, /const run = \+\+localeRun/)
    assert.match(languageSettings, /if \(run !== localeRun\) return/)
    assert.match(languageSettings, /if \(run === localeRun\) \$isChanging\.value = false/)
})

test('the settings page remounts its content when the language changes', () => {
    // The reported defect: switching language on the settings page saved the
    // value ("Saved" appeared) but left the page in the old language until the
    // user navigated away, because the mounted category body never re-resolved.
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
