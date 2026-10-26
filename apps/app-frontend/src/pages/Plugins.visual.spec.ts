import '@/test/visual-styles.scss'

import { mount } from '@vue/test-utils'
import { afterEach, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import PluginSandbox from '@/components/plugins/PluginSandbox.vue'
import {
    LOCAL_PLUGINS_KEY,
    type PluginHost,
    validateLocalPlugin,
} from '@/plugins/local-plugin-runtime'
import { waitFor } from '@/test/visual-harness'

import Plugins from './Plugins.vue'

const fixture = vi.hoisted(() => ({
    instances: vi.fn(async () => [
        {
            id: 'demo',
            name: 'Demo',
            gameVersion: '1.21.1',
            loader: 'fabric',
            selected: true,
            token: 'private',
        },
    ]),
    toast: vi.fn(),
    theme: vi.fn(async () => {}),
    navigate: vi.fn(async () => {}),
    launchDefault: vi.fn(async () => true),
    confirm: vi.fn(async () => true),
}))

vi.mock('@/plugins/use-plugin-host', () => ({ usePluginHost: () => fixture }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: fixture.confirm }))
vi.mock('@modrinth/ui', async () => {
    const { defineComponent, h } = await import('vue')
    return {
        defineMessages: (value: unknown) => value,
        useVIntl: () => ({
            formatMessage: (
                value: { defaultMessage: string },
                args: Record<string, unknown> = {},
            ) =>
                value.defaultMessage.replace(/\{(\w+)\}/gu, (_all, key: string) =>
                    String(args[key] ?? key),
                ),
        }),
        Button: defineComponent({
            inheritAttrs: false,
            props: ['type', 'color', 'disabled'],
            setup:
                (props, { attrs, slots }) =>
                () =>
                    h(
                        'button',
                        { ...attrs, type: 'button', disabled: props.disabled },
                        slots.default?.(),
                    ),
        }),
    }
})

const cleanup: Array<() => void> = []
afterEach(() => cleanup.splice(0).forEach((remove) => remove()))

it('checks sandbox document isolation under the launcher parent script policy', async () => {
    const policy = document.createElement('meta')
    policy.httpEquiv = 'Content-Security-Policy'
    policy.content = "script-src 'self' 'unsafe-eval'; frame-src 'self'"
    document.head.append(policy)
    cleanup.push(() => policy.remove())
    const reports: string[] = []
    const receive = (event: MessageEvent) => {
        if (event.data?.type === 'plugin-policy-result') reports.push(event.data.mode)
    }
    window.addEventListener('message', receive)
    cleanup.push(() => window.removeEventListener('message', receive))
    const plugin = {
        enabled: true,
        manifest: validateLocalPlugin({
            id: 'test.policy',
            name: 'Policy',
            version: '1.0.0',
            page: {
                title: 'Policy',
                html: '<p>Policy</p>',
                script: 'parent.postMessage({type:"plugin-policy-result",mode:"external-bootstrap"},"*");',
            },
        }),
    }
    const wrapper = mount(PluginSandbox, {
        props: { plugin, content: plugin.manifest.page!, host: fixture as PluginHost },
        attachTo: document.body,
    })
    cleanup.push(() => wrapper.unmount())
    await waitFor(() => reports.length === 1, {
        timeoutMs: 3000,
        label: 'fixed external bootstrap under parent CSP',
    })
    expect(reports).toEqual(['external-bootstrap'])
})

it('installs only after permission review, enables a sandbox page, and stops it on disable', async () => {
    localStorage.removeItem(LOCAL_PLUGINS_KEY)
    const wrapper = mount(Plugins, { attachTo: document.body })
    cleanup.push(() => wrapper.unmount())
    const manifest = {
        id: 'test.manager',
        name: 'Local Test',
        version: '1.0.0',
        permissions: ['instances.read', 'ui.theme'],
        theme: { accent: 'ocean' },
        page: { title: 'Local test page', html: '<p>Local page</p>' },
    }
    const file = new File([JSON.stringify(manifest)], 'manifest.json', { type: 'application/json' })
    const transfer = new DataTransfer()
    transfer.items.add(file)
    const picker = wrapper.get<HTMLInputElement>('input[type="file"]')
    picker.element.files = transfer.files
    await picker.trigger('change')
    await waitFor(() => wrapper.find('[data-plugin-review]').exists())
    expect(wrapper.find('[data-plugin-id="test.manager"]').exists()).toBe(false)
    expect(wrapper.get('[data-plugin-review]').text()).toContain('Read instance names and versions')
    await wrapper.get('[data-plugin-install-confirm]').trigger('click')
    expect(wrapper.find('[data-plugin-id="test.manager"]').exists()).toBe(true)
    expect(localStorage.getItem(LOCAL_PLUGINS_KEY)).toContain('test.manager')
    expect(fixture.theme).not.toHaveBeenCalled()
    await wrapper.get('[data-plugin-open="test.manager"]').trigger('click')
    const frame = wrapper.get('iframe')
    expect(frame.attributes('sandbox')).toBe('allow-scripts')
    expect(frame.attributes('referrerpolicy')).toBe('no-referrer')
    expect(frame.attributes('srcdoc')).toContain("connect-src 'none'")
    await wrapper.get('[data-plugin-toggle="test.manager"]').trigger('click')
    expect(wrapper.find('iframe').exists()).toBe(false)
    expect(wrapper.get('[data-plugin-id="test.manager"]').text()).toContain('Disabled')
    await wrapper.get('[data-plugin-toggle="test.manager"]').trigger('click')
    expect(wrapper.find('[data-plugin-review]').exists()).toBe(true)
    await wrapper.get('[data-plugin-install-confirm]').trigger('click')
    expect(wrapper.get('[data-plugin-id="test.manager"]').text()).toContain('Enabled')
})

it('runs a real opaque sandbox bridge, redacts instance data, and blocks fetch and host access', async () => {
    const reports: Record<string, unknown>[] = []
    const receive = (event: MessageEvent) => {
        if (event.data?.type === 'plugin-test-result') reports.push(event.data)
    }
    window.addEventListener('message', receive)
    cleanup.push(() => window.removeEventListener('message', receive))
    const plugin = {
        enabled: true,
        manifest: validateLocalPlugin({
            id: 'test.sandbox',
            name: 'Sandbox',
            version: '1.0.0',
            permissions: ['instances.read'],
            page: {
                title: 'Sandbox',
                html: '<p>Sandbox</p>',
                script: `(async () => {
                const items = await window.mctrin.instances.list();
                let networkBlocked = false;
                let hostBlocked = false;
                try { await fetch('https://example.invalid/private'); } catch { networkBlocked = true; }
                try { parent.document.querySelector('body'); } catch { hostBlocked = true; }
                parent.postMessage({ type: 'plugin-test-result', items, networkBlocked, hostBlocked, nativeAvailable: typeof window.__TAURI__ !== 'undefined' }, '*');
            })();`,
            },
        }),
    }
    const wrapper = mount(PluginSandbox, {
        props: { plugin, content: plugin.manifest.page!, host: fixture as PluginHost },
        attachTo: document.body,
    })
    cleanup.push(() => wrapper.unmount())
    await waitFor(() => reports.length === 1, { timeoutMs: 5000, label: 'sandbox bridge result' })
    expect(reports[0]).toEqual({
        type: 'plugin-test-result',
        items: [
            { id: 'demo', name: 'Demo', gameVersion: '1.21.1', loader: 'fabric', selected: true },
        ],
        networkBlocked: true,
        hostBlocked: true,
        nativeAvailable: false,
    })
    const reads = fixture.instances.mock.calls.length
    window.postMessage(
        {
            type: 'mctrin:request',
            requestId: 'spoof',
            action: 'instances.list',
            session: 'not-the-frame-session',
        },
        '*',
    )
    await nextTick()
    expect(fixture.instances.mock.calls.length).toBe(reads)
})

it('refuses an undeclared bridge permission without calling the platform', async () => {
    const reports: unknown[] = []
    const receive = (event: MessageEvent) => {
        if (event.data?.type === 'plugin-test-denied') reports.push(event.data.message)
    }
    window.addEventListener('message', receive)
    cleanup.push(() => window.removeEventListener('message', receive))
    const plugin = {
        enabled: true,
        manifest: validateLocalPlugin({
            id: 'test.denied',
            name: 'Denied',
            version: '1.0.0',
            permissions: [],
            page: {
                title: 'Denied',
                html: '<p>Denied</p>',
                script: "window.mctrin.instances.list().catch((error) => parent.postMessage({ type: 'plugin-test-denied', message: error.message }, '*'));",
            },
        }),
    }
    const read = vi.fn(async () => [])
    const wrapper = mount(PluginSandbox, {
        props: { plugin, content: plugin.manifest.page!, host: { ...fixture, instances: read } },
        attachTo: document.body,
    })
    cleanup.push(() => wrapper.unmount())
    await waitFor(() => reports.length === 1, { timeoutMs: 5000, label: 'permission denial' })
    expect(reports[0]).toContain('instances.read')
    expect(read).not.toHaveBeenCalled()
})

it('revokes the bridge when the sandbox navigates away from its original document', async () => {
    const plugin = {
        enabled: true,
        manifest: validateLocalPlugin({
            id: 'test.navigation',
            name: 'Navigation',
            version: '1.0.0',
            permissions: [],
            page: {
                title: 'Navigation',
                html: '<p>Navigation</p>',
                script: "setTimeout(() => window.location.replace('about:blank'), 25);",
            },
        }),
    }
    const wrapper = mount(PluginSandbox, {
        props: { plugin, content: plugin.manifest.page!, host: fixture as PluginHost },
        attachTo: document.body,
    })
    cleanup.push(() => wrapper.unmount())
    await waitFor(() => !wrapper.find('iframe').exists(), {
        timeoutMs: 5000,
        label: 'bridge revocation after navigation',
    })
    expect(wrapper.find('iframe').exists()).toBe(false)
})
