import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

import { resolveBreadcrumbLabel } from '../../helpers/breadcrumb-label.ts'

const localeFiles = {
    'en-US': '../../locales/en-US/index.json',
    'zh-CN': '../../locales/zh-CN/index.json',
    'zh-TW': '../../locales/zh-TW/index.json',
} as const

const localeMessages = Object.fromEntries(
    Object.entries(localeFiles).map(([locale, path]) => [
        locale,
        JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8')) as Record<
            string,
            { message: string }
        >,
    ]),
) as Record<keyof typeof localeFiles, Record<string, { message: string }>>

test('upgrade breadcrumb reuses localized Upgrade instance message at runtime', () => {
    let locale: keyof typeof localeFiles = 'en-US'
    const labels = { Upgrade: 'app.instance.upgrade-instance' }
    const resolve = () =>
        resolveBreadcrumbLabel(
            'Upgrade',
            () => '',
            labels,
            (messageId) => localeMessages[locale][messageId].message,
        )

    assert.equal(resolve(), 'Upgrade instance')
    locale = 'zh-CN'
    assert.equal(resolve(), '升级实例')
    locale = 'zh-TW'
    assert.equal(resolve(), '升級實例')
})

test('screenshots breadcrumb resolves through the active locale', () => {
    let locale: 'en-US' | 'zh-CN' = 'en-US'
    const labels = { Screenshots: 'app.navigation.screenshots' }
    const resolve = () =>
        resolveBreadcrumbLabel(
            'Screenshots',
            () => '',
            labels,
            (messageId) => localeMessages[locale][messageId].message,
        )

    assert.equal(resolve(), 'Screenshots')
    locale = 'zh-CN'
    assert.equal(resolve(), '截图中心')
})

test('upgrade route paths, internal names, and breadcrumb depth stay unchanged', () => {
    const routes = readFileSync(new URL('../../routes/instance.ts', import.meta.url), 'utf8')
    assert.match(routes, /useRootContext: true,[\s\S]*?breadcrumb: \[\{ name: 'Upgrade' \}\]/)
    assert.doesNotMatch(routes, /breadcrumb: \[\{ name: '\?Instance'[^\]]*\{ name: 'Upgrade' \}\]/)
    for (const [path, name] of [
        ['', 'InstanceUpgrade'],
        ['compatibility', 'InstanceUpgradeCompatibility'],
        ['customize', 'InstanceUpgradeCustomize'],
        ['confirm', 'InstanceUpgradeConfirm'],
        ['progress', 'InstanceUpgradeProgress'],
        ['result', 'InstanceUpgradeResult'],
    ] as const) {
        assert.match(routes, new RegExp(`path: '${path}',\\s+name: '${name}'`))
    }
})

test('breadcrumb component resolves Upgrade through formatMessage on each render', () => {
    const source = readFileSync(new URL('./Breadcrumbs.vue', import.meta.url), 'utf8')
    assert.match(source, /Upgrade: messages\.upgradeInstance/)
    assert.match(source, /Screenshots: messages\.screenshots/)
    assert.match(source, /Upgrade: ArrowBigUpDashIcon/)
    assert.match(source, /screenshots: \{ id: 'app\.navigation\.screenshots'/)
    assert.match(source, /id: 'app\.instance\.upgrade-instance'/)
    assert.match(source, /resolveBreadcrumbLabel\([\s\S]*?\(message\) => formatMessage\(message\)/)
})

test('every static route breadcrumb resolves through a localized message', () => {
    // The default zh-CN chrome previously showed English breadcrumbs because
    // these route names fell through to the raw English route label.
    const labels: Record<string, string> = {
        'AI assistant': 'app.navigation.ai-assistant',
        Plugins: 'app.plugins.title',
        'Command generator': 'app.lab.command-generator.title',
        Versions: 'project.versions.title',
        Gallery: 'project.gallery.title',
    }
    for (const name of Object.keys(labels)) {
        const resolve = (locale: 'en-US' | 'zh-CN') =>
            resolveBreadcrumbLabel(
                name,
                () => '',
                labels,
                (messageId) => localeMessages[locale][messageId].message,
            )
        assert.notEqual(resolve('zh-CN'), name, `${name} must not stay English in zh-CN`)
        assert.notEqual(resolve('zh-CN'), resolve('en-US'), `${name} must differ from English`)
    }
    assert.equal(
        resolveBreadcrumbLabel(
            'AI assistant',
            () => '',
            labels,
            (id) => localeMessages['zh-CN'][id].message,
        ),
        'AI 助手',
    )

    // The runtime map above only proves the catalogue has the strings. These
    // assertions prove the component actually consults them, which is what was
    // missing: the route names fell through to the raw English label.
    const source = readFileSync(new URL('./Breadcrumbs.vue', import.meta.url), 'utf8')
    for (const [name, messageKey] of [
        ['AI assistant', 'aiAssistant'],
        ['Plugins', 'plugins'],
        ['Command generator', 'commandGenerator'],
        ['Versions', 'versions'],
        ['Gallery', 'gallery'],
    ] as const) {
        assert.match(
            source,
            new RegExp(`'?${name}'?: messages\\.${messageKey},`),
            `${name} must be mapped to a message descriptor`,
        )
    }
})

test('breadcrumb separators render only between visible breadcrumb items', () => {
    const source = readFileSync(new URL('./Breadcrumbs.vue', import.meta.url), 'utf8')
    assert.match(source, /v-for="\(breadcrumb, index\) in breadcrumbs"/)
    assert.match(source, /v-if="index < breadcrumbs\.length - 1"/)
    assert.doesNotMatch(source, /ChevronRightIcon v-if="breadcrumb\.link"/)
})
