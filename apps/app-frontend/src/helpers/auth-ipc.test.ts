import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

test('all frontend authentication calls have runtime handlers, build permissions and main-window access', () => {
    const frontend = readFileSync(new URL('./auth.js', import.meta.url), 'utf8')
    const runtime = readFileSync(new URL('../../../app/src/api/auth.rs', import.meta.url), 'utf8')
    const build = readFileSync(new URL('../../../app/build.rs', import.meta.url), 'utf8')
    const capabilities = JSON.parse(
        readFileSync(new URL('../../../app/capabilities/plugins.json', import.meta.url), 'utf8'),
    )
    const main = readFileSync(new URL('../../../app/src/main.rs', import.meta.url), 'utf8')
    const handlers = runtime.match(/generate_handler!\[([\s\S]*?)\]/)?.[1]
    const permissions = build.match(/"auth",[\s\S]*?\.commands\(&\[([\s\S]*?)\]/)?.[1]
    assert.ok(handlers)
    assert.ok(permissions)
    for (const [, command] of frontend.matchAll(/invoke\('plugin:auth\|([a-z_]+)'/g)) {
        assert.ok(
            handlers.split(',').some((name) => name.trim() === command),
            `Missing runtime handler: ${command}`,
        )
        assert.ok(permissions.includes(`"${command}"`), `Missing build permission: ${command}`)
    }
    assert.ok(capabilities.windows.includes('main'))
    assert.ok(capabilities.permissions.includes('auth:default'))
    assert.ok(main.includes('.plugin(api::auth::init())'))
})
