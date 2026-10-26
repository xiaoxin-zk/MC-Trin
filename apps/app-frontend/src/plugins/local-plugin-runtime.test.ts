import assert from 'node:assert/strict'
import test from 'node:test'

import {
    buildLocalPluginDocument,
    dispatchLocalPluginRequest,
    type InstalledLocalPlugin,
    isPluginRequest,
    parseLocalPlugin,
    pluginAccentToLauncher,
    type PluginHost,
    type PluginRequest,
    readLocalPlugins,
    runLocalPluginAction,
    validateLocalPlugin,
    writeLocalPlugins,
} from './local-plugin-runtime.ts'

const minimal = () => ({
    id: 'example.notes',
    name: 'Notes',
    version: '1.0.0',
    permissions: [],
    page: { title: 'Notes', html: '<h1>Notes</h1>' },
})
const installed = (value: unknown = minimal()): InstalledLocalPlugin => ({
    manifest: validateLocalPlugin(value),
    enabled: true,
})
const host = (changes: Partial<PluginHost> = {}): PluginHost => ({
    instances: async () => [],
    toast: () => {},
    theme: async () => {},
    navigate: async () => {},
    launchDefault: async () => true,
    ...changes,
})
const request = (changes: Partial<PluginRequest> = {}): PluginRequest => ({
    type: 'mctrin:request',
    requestId: '1',
    session: '0123456789abcdef',
    action: 'instances.list',
    ...changes,
})

test('accepts the original local manifest format and reconstructs only allowlisted fields', () => {
    const manifest = parseLocalPlugin(
        JSON.stringify({
            ...minimal(),
            path: 'C:/secret',
            endpoint: 'https://remote.test',
            page: { ...minimal().page, src: 'https://remote.test/code' },
        }),
    )
    assert.deepEqual(manifest.permissions, [])
    assert.deepEqual(manifest.panels, [])
    assert.equal(Object.prototype.hasOwnProperty.call(manifest, 'path'), false)
    assert.equal(Object.prototype.hasOwnProperty.call(manifest.page!, 'src'), false)
})

test('rejects unsupported host permissions, duplicate permissions, and non-array permission fields', () => {
    for (const permissions of [['fs.read'], ['ui.toast', 'ui.toast'], 'instances.read'])
        assert.throws(() => validateLocalPlugin({ ...minimal(), permissions }))
})

test('requires the matching permission and a declared built-in navigation target', () => {
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                actions: [{ id: 'say', label: 'Say', type: 'toast', value: 'Hello' }],
            }),
        /ui.toast/,
    )
    for (const value of ['https://remote.test', '../../files', '/instance/private', '__proto__'])
        assert.throws(() =>
            validateLocalPlugin({
                ...minimal(),
                permissions: ['app.navigate'],
                actions: [{ id: 'go', label: 'Go', type: 'navigate', value }],
            }),
        )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                permissions: ['ui.toast'],
                actions: [{ id: 'run', label: 'Run', type: '__proto__' }],
            }),
        /Unsupported/,
    )
})

test('rejects duplicate and excessive actions', () => {
    const action = { id: 'say', label: 'Say', type: 'toast', value: 'Hello' }
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                permissions: ['ui.toast'],
                actions: [action, action],
            }),
        /Duplicate/,
    )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                permissions: ['ui.toast'],
                actions: Array.from({ length: 13 }, (_, i) => ({ ...action, id: `say${i}` })),
            }),
        /at most 12/,
    )
})

test('HTML cannot replace the document policy or smuggle executable handlers', () => {
    for (const html of [
        '<script>alert(1)</script>',
        '<meta http-equiv="refresh" content="0;url=https://remote.test">',
        '<iframe srcdoc="anything"></iframe>',
        '<img src=x onerror="alert(1)">',
        '<svg onload = "alert(1)"></svg>',
        '<a href="javascript:alert(1)">link</a>',
        '<form action="https://remote.test"></form>',
        '</body><body>escape',
    ])
        assert.throws(
            () => validateLocalPlugin({ ...minimal(), page: { title: 'Unsafe', html } }),
            /unsupported|cannot/,
        )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                page: { ...minimal().page, html: 'a'.repeat(500_001) },
            }),
        /500 KB/,
    )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                page: { ...minimal().page, script: 'a'.repeat(100_001) },
            }),
        /100 KB/,
    )
})

test('theme and home panel hooks are bounded and use existing accent mappings', () => {
    const manifest = validateLocalPlugin({
        ...minimal(),
        permissions: ['ui.theme'],
        theme: { accent: 'ocean', mode: 'oled' },
        panels: [
            { id: 'notes', title: 'Notes', slot: 'home.after', html: '<p>Saved</p>', height: 160 },
        ],
    })
    assert.equal(manifest.theme?.mode, 'oled')
    assert.equal(pluginAccentToLauncher(manifest.theme!.accent), 'blue')
    assert.equal(manifest.panels[0].slot, 'home.after')
    assert.throws(
        () => validateLocalPlugin({ ...minimal(), theme: { accent: 'ocean' } }),
        /ui.theme/,
    )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                panels: [{ id: 'x', title: 'Bad', slot: 'host.html', html: '' }],
            }),
        /Unsupported/,
    )
    assert.throws(
        () =>
            validateLocalPlugin({
                ...minimal(),
                panels: [{ id: 'x', title: 'Big', slot: 'home.after', html: '', height: 601 }],
            }),
        /height/,
    )
})

test('persistence round-trips local manifests and skips corrupt or duplicated records', () => {
    const plugin = installed()
    const json = writeLocalPlugins([plugin])
    assert.deepEqual(readLocalPlugins(json), [plugin])
    assert.deepEqual(readLocalPlugins('bad json'), [])
    assert.deepEqual(readLocalPlugins('{}'), [])
    assert.deepEqual(
        readLocalPlugins(
            JSON.stringify([
                { manifest: { id: '../bad' }, enabled: true },
                plugin,
                { ...plugin, enabled: false },
            ]),
        ),
        [plugin],
    )
    assert.throws(() => writeLocalPlugins(Array(41).fill(plugin)), /40/)
})

test('bridge envelopes bind requests to a nonce session and supported request names', () => {
    assert.equal(isPluginRequest(request(), '0123456789abcdef'), true)
    for (const candidate of [
        { ...request(), session: 'stale' },
        { ...request(), requestId: 'x'.repeat(65) },
        { ...request(), action: 'native.invoke' },
        { ...request(), action: 'actions.run', actionId: '../../secret' },
    ])
        assert.equal(isPluginRequest(candidate, '0123456789abcdef'), false)
})

test('instance results redact extra fields and reject undeclared permission before reading', async () => {
    let reads = 0
    const platform = host({
        instances: async () => {
            reads++
            return [
                {
                    id: 'i1',
                    name: 'Test',
                    gameVersion: '1.21',
                    loader: 'fabric',
                    selected: true,
                    token: 'secret',
                    path: 'C:/secret',
                } as never,
            ]
        },
    })
    const denied = await dispatchLocalPluginRequest(installed(), request(), platform)
    assert.equal(denied.ok, false)
    assert.equal(reads, 0)
    const allowed = await dispatchLocalPluginRequest(
        installed({ ...minimal(), permissions: ['instances.read'] }),
        request(),
        platform,
    )
    assert.equal(allowed.ok, true)
    assert.deepEqual(allowed.value, [
        { id: 'i1', name: 'Test', gameVersion: '1.21', loader: 'fabric', selected: true },
    ])
    assert.doesNotMatch(JSON.stringify(allowed), /secret|token|path/)
})

test('disabled plugins and unknown action IDs cannot call host capabilities', async () => {
    let calls = 0
    const plugin = installed({
        ...minimal(),
        permissions: ['ui.toast'],
        actions: [{ id: 'say', label: 'Say', type: 'toast', value: 'Hello' }],
    })
    const platform = host({
        toast: () => {
            calls++
        },
    })
    await assert.rejects(runLocalPluginAction(plugin, 'undeclared', platform), /Unknown/)
    plugin.enabled = false
    await assert.rejects(runLocalPluginAction(plugin, 'say', platform), /disabled/)
    const response = await dispatchLocalPluginRequest(plugin, request(), platform)
    assert.equal(response.ok, false)
    assert.equal(calls, 0)
})

test('declared actions call exactly their permitted capability and launch decline remains a failure', async () => {
    const calls: unknown[] = []
    const plugin = installed({
        ...minimal(),
        permissions: ['ui.toast', 'ui.theme', 'app.navigate', 'game.launchDefault'],
        actions: [
            { id: 'say', label: 'Say', type: 'toast', value: 'Hello' },
            { id: 'style', label: 'Style', type: 'theme', value: 'ocean' },
            { id: 'home', label: 'Home', type: 'navigate', value: 'home' },
            { id: 'launch', label: 'Launch', type: 'launchDefault' },
        ],
    })
    const platform = host({
        toast: (text) => calls.push(text),
        theme: async (theme) => {
            calls.push(theme)
        },
        navigate: async (target) => {
            calls.push(target)
        },
        launchDefault: async () => false,
    })
    await runLocalPluginAction(plugin, 'say', platform)
    await runLocalPluginAction(plugin, 'style', platform)
    await runLocalPluginAction(plugin, 'home', platform)
    const response = await dispatchLocalPluginRequest(
        plugin,
        request({ action: 'actions.run', actionId: 'launch' }),
        platform,
    )
    assert.deepEqual(calls, ['Hello', { accent: 'ocean' }, 'home'])
    assert.equal(response.ok, false)
    assert.match(String(response.value), /declined/)
})

test('sandbox document permits only the fixed bootstrap and keeps plugin source in inert JSON', () => {
    const html = buildLocalPluginDocument(
        {
            title: 'Notes',
            html: '<p>Notes</p>',
            script: 'const close = "</script><script>escaped</script>";',
        },
        '0123456789abcdef',
        'https://mctrin.localhost/mctrin-plugin-bootstrap.js',
    )
    assert.match(
        html,
        /script-src https:\/\/mctrin.localhost\/mctrin-plugin-bootstrap.js 'unsafe-eval'/,
    )
    assert.match(html, /connect-src 'none'/)
    assert.match(html, /base-uri 'none'/)
    assert.doesNotMatch(html, /__TAURI__|allow-same-origin|<script>escaped/)
    assert.match(html, /type="application\/json"/)
    assert.match(html, /\\u003c\/script\\u003e/)
    assert.equal((html.match(/<script src=/gu) ?? []).length, 1)
    assert.throws(
        () =>
            buildLocalPluginDocument(
                minimal().page,
                'bad"nonce',
                'https://mctrin.localhost/mctrin-plugin-bootstrap.js',
            ),
        /Invalid/,
    )
    assert.throws(
        () =>
            buildLocalPluginDocument(
                minimal().page,
                '0123456789abcdef',
                'https://remote.test/arbitrary.js',
            ),
        /Invalid/,
    )
})
