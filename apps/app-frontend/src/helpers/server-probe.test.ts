import assert from 'node:assert/strict'
import test from 'node:test'

import { classifyServerProbeError, pruneServerData, serverProbeState } from './server-probe.ts'
import type { ServerData, World } from './worlds.ts'

test('distinguishes a reachable server, a network timeout, and a failed probe', () => {
    assert.equal(serverProbeState({ refreshing: true }), 'checking')
    assert.equal(
        serverProbeState({ refreshing: false, status: { enforces_secure_chat: false } }),
        'online',
    )
    assert.equal(classifyServerProbeError(new Error('Ping timed out')), 'offline')
    assert.equal(classifyServerProbeError(new Error('invalid status packet')), 'error')
})

test('prunes probe data for removed and address-changed servers', () => {
    const cache: Record<string, ServerData> = {
        'old.example': { refreshing: false },
        'active.example': { refreshing: false },
    }
    const worlds: World[] = [
        {
            type: 'server',
            name: 'Active',
            display_status: 'normal',
            index: 0,
            address: 'active.example',
            pack_status: 'prompt',
        },
    ]

    pruneServerData(cache, worlds)

    assert.deepEqual(Object.keys(cache), ['active.example'])
})
