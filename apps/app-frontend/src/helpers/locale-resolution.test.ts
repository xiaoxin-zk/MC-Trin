import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

import {
    DEFAULT_LOCALE,
    matchLocale,
    normalizeLocaleTag,
    resolveLocale,
    resolvePreferredLocale,
} from './locale-resolution.ts'

/**
 * Mirrors the runtime list: every directory that also has a `LOCALES` entry. The
 * real list is derived from the same two sources in `i18n.config.ts`.
 */
const availableLocales = [
    'de-CH',
    'de-DE',
    'en-US',
    'es-419',
    'es-ES',
    'fil-PH',
    'pt-BR',
    'pt-PT',
    'tr-TR',
    'zh-CN',
    'zh-TW',
]

test('normalizes separators and case before comparing', () => {
    assert.equal(normalizeLocaleTag('ZH_CN'), 'zh-cn')
    assert.equal(normalizeLocaleTag('  de-DE  '), 'de-de')
    assert.equal(normalizeLocaleTag(''), '')
})

test('matches an exact catalogue regardless of case and separator style', () => {
    assert.equal(matchLocale('zh-CN', availableLocales), 'zh-CN')
    assert.equal(matchLocale('zh_cn', availableLocales), 'zh-CN')
    assert.equal(matchLocale('ZH-cn', availableLocales), 'zh-CN')
    assert.equal(matchLocale('de-DE', availableLocales), 'de-DE')
})

test('maps Traditional Chinese variants to zh-TW and never to zh-CN', () => {
    // The defect this guards: selecting Simplified Chinese used to activate
    // zh-TW, so every label rendered in Traditional Chinese. Reverting the
    // script/region branch makes all four assertions below resolve to zh-CN.
    for (const tag of ['zh-Hant', 'zh-Hant-TW', 'zh-TW', 'zh-HK', 'zh-MO']) {
        assert.equal(matchLocale(tag, availableLocales), 'zh-TW', `${tag} must select zh-TW`)
    }
    assert.equal(matchLocale('zh-TW', ['en-US']), undefined)
})

test('maps Simplified Chinese variants and bare zh to zh-CN', () => {
    for (const tag of ['zh-CN', 'zh-Hans', 'zh-Hans-CN', 'zh-SG', 'zh']) {
        assert.equal(matchLocale(tag, availableLocales), 'zh-CN', `${tag} must select zh-CN`)
    }
})

test('matches a language-only tag to the first shipped regional catalogue', () => {
    assert.equal(matchLocale('de', availableLocales), 'de-CH')
    assert.equal(matchLocale('pt', availableLocales), 'pt-BR')
    assert.equal(matchLocale('es', availableLocales), 'es-419')
})

test('falls back to en-US instead of an arbitrary catalogue', () => {
    // Ordering matters here: an implementation that returned `available[0]`
    // would answer de-CH and reintroduce the mixed-language defect.
    assert.equal(resolveLocale('not-a-locale', availableLocales), DEFAULT_LOCALE)
    assert.equal(resolveLocale('', availableLocales), DEFAULT_LOCALE)
    assert.equal(resolveLocale(null, availableLocales), DEFAULT_LOCALE)
    assert.equal(resolveLocale(undefined, availableLocales), DEFAULT_LOCALE)
    assert.equal(resolveLocale('tr-TR', ['en-US', 'zh-CN']), DEFAULT_LOCALE)
})

test('resolves the first satisfied preference from a language list', () => {
    assert.equal(resolvePreferredLocale(['fr-FR', 'de-DE', 'en-US'], availableLocales), 'de-DE')
    assert.equal(resolvePreferredLocale(['zh-Hant-HK'], availableLocales), 'zh-TW')
    assert.equal(resolvePreferredLocale([], availableLocales), DEFAULT_LOCALE)
    assert.equal(resolvePreferredLocale(['xx-YY'], availableLocales), DEFAULT_LOCALE)
})

test('every selectable code exists in both catalogue trees', () => {
    // `buildLocaleMessages` drops codes missing from `LOCALES`; loading such a
    // directory yields an empty bundle and vue-i18n then mixes languages.
    const appLocales = readFileSync(new URL('../i18n.config.ts', import.meta.url), 'utf8')
    assert.match(appLocales, /LOCALES\.some\(\(definition\) => definition\.code === locale\)/)

    for (const code of availableLocales) {
        for (const tree of ['../locales', '../../../../packages/ui/src/locales']) {
            const path = new URL(`${tree}/${code}/index.json`, import.meta.url)
            const entries = Object.keys(JSON.parse(readFileSync(path, 'utf8')) as object)
            assert.ok(entries.length > 0, `${code} must ship a non-empty ${tree} bundle`)
        }
    }
})
