import assert from 'node:assert/strict'
import { test } from 'node:test'

import type { McpServerConfig, McpServerConfigStorage } from './mcp-server-config.ts'
import {
    MCP_SERVER_CONFIG_STORAGE_KEY,
    readMcpServerConfigs,
    removeMcpServerConfig,
    upsertMcpServerConfig,
    writeMcpServerConfigs,
} from './mcp-server-config.ts'

function createStorage(initial?: string): McpServerConfigStorage & { value: string | null } {
    return {
        value: initial ?? null,
        getItem(key) {
            assert.equal(key, MCP_SERVER_CONFIG_STORAGE_KEY)
            return this.value
        },
        setItem(key, value) {
            assert.equal(key, MCP_SERVER_CONFIG_STORAGE_KEY)
            this.value = value
        },
    }
}

function config(overrides: Partial<McpServerConfig> = {}): McpServerConfig {
    return {
        id: 'server-1',
        name: 'Local tools',
        endpoint: 'https://mcp.example.test/mcp',
        ...overrides,
    }
}

test('accepts HTTPS and the three permitted local HTTP hosts', () => {
    const storage = createStorage()
    const configs = writeMcpServerConfigs(storage, [
        config(),
        config({ id: 'localhost', endpoint: 'http://localhost:8123/mcp' }),
        config({ id: 'ipv4', endpoint: 'http://127.0.0.1:8123/mcp' }),
        config({ id: 'ipv6', endpoint: 'http://[::1]:8123/mcp' }),
    ])

    assert.equal(configs.length, 4)
    assert.equal(configs[3].endpoint, 'http://[::1]:8123/mcp')
})

test('rejects non-local HTTP, unsupported protocols, user information, fragments, and invalid URLs', () => {
    for (const endpoint of [
        'http://example.test/mcp',
        'http://127.1:8123/mcp',
        'ftp://mcp.example.test',
        'https://user:secret@mcp.example.test',
        'https://@mcp.example.test',
        'https://mcp.example.test/mcp#section',
        'https://mcp.example.test/mcp#',
        'not a URL',
    ]) {
        assert.throws(() => writeMcpServerConfigs(createStorage(), [config({ endpoint })]))
    }
})

test('trims names and enforces the 1 to 64 character limit', () => {
    const storage = createStorage()
    const [saved] = writeMcpServerConfigs(storage, [config({ name: '  Tools  ' })])
    assert.equal(saved.name, 'Tools')

    for (const name of ['', '   ', 'x'.repeat(65)]) {
        assert.throws(() => writeMcpServerConfigs(createStorage(), [config({ name })]))
    }
})

test('invalid or duplicate stored data falls back to an empty list', () => {
    const duplicate = [
        config({ id: 'first', endpoint: 'https://MCP.example.test:443/mcp' }),
        config({ id: 'second', endpoint: 'https://mcp.example.test/mcp' }),
    ]
    for (const raw of [
        '{broken',
        '{}',
        JSON.stringify([config({ name: '' })]),
        JSON.stringify(duplicate),
    ]) {
        assert.deepEqual(readMcpServerConfigs(createStorage(raw)), [])
    }
    assert.deepEqual(readMcpServerConfigs(createStorage()), [])
})

test('upsert updates by id and stores only id, name, and endpoint', () => {
    const storage = createStorage()
    upsertMcpServerConfig(storage, config({ name: 'Original' }))
    upsertMcpServerConfig(storage, {
        ...config({ name: 'Updated' }),
        bearerToken: 'must-not-persist',
    } as McpServerConfig)

    assert.deepEqual(readMcpServerConfigs(storage), [config({ name: 'Updated' })])
    assert.doesNotMatch(storage.value ?? '', /bearerToken|must-not-persist/)
    assert.throws(() =>
        upsertMcpServerConfig(
            storage,
            config({ id: 'different-id', endpoint: 'https://MCP.example.test:443/mcp' }),
        ),
    )
})

test('remove deletes by id and leaves unrelated entries intact', () => {
    const storage = createStorage()
    writeMcpServerConfigs(storage, [
        config(),
        config({ id: 'server-2', name: 'Other', endpoint: 'https://other.example.test/mcp' }),
    ])

    assert.deepEqual(removeMcpServerConfig(storage, 'server-1'), [
        config({ id: 'server-2', name: 'Other', endpoint: 'https://other.example.test/mcp' }),
    ])
    assert.deepEqual(readMcpServerConfigs(storage), [
        config({ id: 'server-2', name: 'Other', endpoint: 'https://other.example.test/mcp' }),
    ])
})
