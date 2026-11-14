import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

import { resolveSettingsTarget, settingsTargetRoute } from './settings-target.ts'

const read = (file: string) =>
    readFileSync(new URL(`./${file}`, import.meta.url), 'utf8')

test('builds and reads back a scroll target inside one settings category', () => {
    const route = settingsTargetRoute('launch-defaults', 'log-share')
    assert.equal(route.path, '/settings')
    assert.equal(route.hash, '#launch-defaults')
    assert.equal(route.query?.target, 'log-share')
    assert.deepEqual(resolveSettingsTarget(route.hash, route.query ?? {}), {
        categoryId: 'launch-defaults',
        targetId: 'settings-target-log-share',
    })
})

test('ignores a target that is not a plain identifier', () => {
    assert.equal(settingsTargetRoute('launch-defaults', '../etc/passwd').query, undefined)
    assert.equal(resolveSettingsTarget('#launch-defaults', { target: '#logs' }), null)
    assert.equal(resolveSettingsTarget('#launch-defaults', {}), null)
})

test('the log-diagnosis target opens a category that owns the AI model controls', () => {
    // The defect this pins: the assistant's log-mode affordance pointed at the
    // `logs` category, whose page has no AI controls at all, so "Configure AI"
    // opened a page where nothing could be configured. The anchor the button
    // builds must resolve inside the category that really renders the log
    // analysis provider and model pickers.
    const registry = read('settings-registry.ts')
    const category = (id: string) => {
        const match = new RegExp(`'?${id}'?:\\s*\\{([\\s\\S]*?)\\n    \\},`).exec(registry)
        assert.ok(match, `settings-registry must define the ${id} category`)
        return match[1]
    }
    const componentFile = (id: string) => {
        const match = /import\('\.\/([\w.-]+\.vue)'\)/.exec(category(id))
        assert.ok(match, `${id} must load a settings component`)
        return match[1]
    }

    const logCategory = componentFile('launch-defaults')
    assert.equal(logCategory, 'DefaultInstanceSettings.vue')
    const logCategorySource = read(logCategory)
    assert.match(
        logCategorySource,
        /<LogShareSettings\s*\/>/,
        'the log analysis settings must be rendered by the launch-defaults category',
    )
    const logShare = read('LogShareSettings.vue')
    assert.match(logShare, /id="settings-target-log-share"/)
    for (const control of ['crash-analysis-ai-provider', 'crash-analysis-ai-model']) {
        assert.match(logShare, new RegExp(`id="${control}"`), `${control} must exist`)
    }

    // And the old destination must stay empty of them, so the regression cannot
    // come back as "it still works, the page just got busy".
    const logsCategory = componentFile('logs')
    assert.equal(logsCategory, 'LogsSettings.vue')
    const logsSource = read(logsCategory)
    assert.doesNotMatch(logsSource, /crash-analysis-ai-(provider|model|enabled)/)
    assert.doesNotMatch(logsSource, /settings-target-log-share/)
})
