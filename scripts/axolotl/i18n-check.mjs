import { readFile, readdir } from 'node:fs/promises'
import path from 'node:path'

import { parse, TYPE } from '@formatjs/icu-messageformat-parser'

const localePairs = [
    [
        'apps/app-frontend/src/locales/en-US/index.json',
        'apps/app-frontend/src/locales/zh-CN/index.json',
    ],
    ['packages/ui/src/locales/en-US/index.json', 'packages/ui/src/locales/zh-CN/index.json'],
]

// Which message-id prefix belongs to which locale pair, so a component's id is
// only required in the locale files that actually serve it.
const localePrefixes = {
    'apps/app-frontend/src/locales/en-US/index.json': 'app.',
    'packages/ui/src/locales/en-US/index.json': 'app.',
}

const sourceRoots = ['apps/app-frontend/src', 'packages/ui/src']
const sourceExtensions = new Set(['.vue', '.ts', '.tsx', '.js', '.mjs'])
const ignoredDirectories = new Set([
    'node_modules',
    '.vite',
    'dist',
    '.output',
    '__screenshots__',
    'public',
    'locales',
])

async function collectSources(directory, files) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
        if (entry.isDirectory()) {
            if (ignoredDirectories.has(entry.name)) continue
            await collectSources(path.join(directory, entry.name), files)
            continue
        }
        if (sourceExtensions.has(path.extname(entry.name))) {
            files.push(path.join(directory, entry.name))
        }
    }
    return files
}

const failures = []
const allowedUntranslatedMessages = new Set([
    'Chaos Cubed',
    'MINECON Earth 2017',
    'Modrinth',
    '.minecraft',
    'Striding Hero',
    'Axolotl Launcher',
    'Explore high-quality Minecraft content on Modrinth.',
    'example.modrinth.gg',
    '{title} - {count}',
    'Hooks',
    '/path/to/java',
    'https://example.com/api/yggdrasil',
    'Fabric',
    'Forge',
    'Paper',
    'Cleanroom',
    'LiteLoader',
    'LogShare.CN',
    'LogShare AI',
    'NeoForge',
    'OptiFine',
    'Quilt',
    'Mr Pack',
    'TNT',
    'CurseForge',
    'BBCode',
    'mclo.gs',
    'CMI',
    'CSV',
    'AI',
    'HTML',
    'Mojang G1GC',
    'PCL G1GC',
    'PCL',
    'Shenandoah',
    'ZGC',
    'MineDown',
    'MiniMessage',
    'TabooLib',
    'TrChat',
    'TNT',
    'Java {version}',
    'Studio',
    'https://api-free.deepl.com/v2/translate',
    'Java',
    'Minecraft EULA',
    'server.properties',
    '{type} · {version}',
    '{value} MB',
    '> {command}',
    'Beta',
    'Release',
    'DNS over HTTPS',
    'https://doh.pub/dns-query',
    '{name} · {duration}',
    '{name} {level}',
    '{name} {number} · {x}, {z}',
    '{biome} · {name}',
    '{degrees}°',
])

function messageText(value) {
    return typeof value === 'string' ? value : (value?.message ?? value?.defaultMessage ?? '')
}

function hasExplicitTranslation(value) {
    return typeof value === 'string' || typeof value?.message === 'string'
}

function argumentNames(message) {
    const names = new Set()
    const argumentTypes = new Set([
        TYPE.argument,
        TYPE.number,
        TYPE.date,
        TYPE.time,
        TYPE.select,
        TYPE.plural,
    ])

    function visit(elements) {
        for (const element of elements) {
            if (argumentTypes.has(element.type)) names.add(element.value)
            if (element.options) {
                for (const option of Object.values(element.options)) visit(option.value)
            }
            if (element.children) visit(element.children)
        }
    }

    visit(parse(message))
    return [...names].sort()
}

for (const [sourcePath, translationPath] of localePairs) {
    const source = JSON.parse(await readFile(sourcePath, 'utf8'))
    const translation = JSON.parse(await readFile(translationPath, 'utf8'))

    for (const key of Object.keys(source)) {
        if (!(key in translation)) {
            failures.push(`${translationPath}: missing ${key}`)
            continue
        }

        try {
            const sourceMessage = messageText(source[key])
            const translationMessage = messageText(translation[key])
            const sourceArguments = argumentNames(sourceMessage)
            const translationArguments = argumentNames(translationMessage)
            if (sourceArguments.join('\0') !== translationArguments.join('\0')) {
                failures.push(
                    `${translationPath}: ICU arguments for ${key} are [${translationArguments.join(', ')}], expected [${sourceArguments.join(', ')}]`,
                )
            }

            if (
                sourceMessage === translationMessage &&
                hasExplicitTranslation(translation[key]) &&
                /[A-Za-z]{2}/.test(sourceMessage) &&
                !allowedUntranslatedMessages.has(sourceMessage)
            ) {
                failures.push(`${translationPath}: untranslated ${key}`)
            }
        } catch (error) {
            failures.push(`${translationPath}: invalid ICU message ${key}: ${error.message}`)
        }
    }
}

// The locale files are only compared with each other, so a message that is
// referenced from a component but never added to either locale file stays
// invisible here: English and Simplified Chinese simply both lack it, and the
// UI falls back to the `defaultMessage`. Collect every `id:` a source file
// declares and require an entry in both locale files.
//
// The repository carries a long tail of legacy declarations that were never
// added to the locale files, so a full sweep would bury new breakage in
// hundreds of pre-existing lines. Those live in the baseline below; a
// declaration that is missing today and not listed there is a regression.
const declaredMessageIds = new Set()
const declarationPattern = /id:\s*'(app\.[a-zA-Z0-9.-]+)'/gu
for (const root of ['apps/app-frontend/src', 'packages/ui/src']) {
    for (const file of await collectSources(root, [])) {
        for (const match of (await readFile(file, 'utf8')).matchAll(declarationPattern)) {
            declaredMessageIds.add(match[1])
        }
    }
}
const legacyMissing = new Set(await readFile(
    'scripts/axolotl/i18n-declared-baseline.txt', 'utf8',
).then((text) => text.split('\n').map((line) => line.trim()).filter(Boolean)))
for (const [sourcePath, translationPath] of localePairs) {
    const source = JSON.parse(await readFile(sourcePath, 'utf8'))
    const translation = JSON.parse(await readFile(translationPath, 'utf8'))
    for (const id of declaredMessageIds) {
        if (!id.startsWith(localePrefixes[sourcePath])) continue
        const inSource = id in source
        const inTranslation = id in translation
        if (inSource && inTranslation) continue
        if (legacyMissing.has(id)) continue
        if (!inSource) {
            failures.push(`${sourcePath}: missing ${id} that a component declares`)
        }
        if (!inTranslation) {
            failures.push(`${translationPath}: missing ${id} that a component declares`)
        }
    }
}

if (failures.length > 0) {
    console.error(`Simplified Chinese coverage check failed:\n${failures.join('\n')}`)
    process.exit(1)
}

console.log('Simplified Chinese key coverage and ICU argument checks passed.')
