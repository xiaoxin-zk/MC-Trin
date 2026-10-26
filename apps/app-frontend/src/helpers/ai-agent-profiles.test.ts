import assert from 'node:assert/strict'
import test from 'node:test'

import {
    addAgentProfile,
    AI_AGENT_PROFILES_STORAGE_KEY,
    getAgentProfiles,
    removeAgentProfile,
    updateAgentProfile,
    DEFAULT_AI_AGENT_PROFILE,
} from './ai-agent-profiles.ts'

function memoryStorage(initial?: string) {
    const values = new Map<string, string>()
    if (initial !== undefined) values.set(AI_AGENT_PROFILES_STORAGE_KEY, initial)

    return {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
    }
}

test('returns the built-in assistant when storage is empty or malformed', () => {
    assert.deepEqual(getAgentProfiles(memoryStorage()), [DEFAULT_AI_AGENT_PROFILE])
    assert.deepEqual(getAgentProfiles(memoryStorage('{broken json')), [DEFAULT_AI_AGENT_PROFILE])
    assert.deepEqual(getAgentProfiles(memoryStorage(JSON.stringify({ profiles: [] }))), [
        DEFAULT_AI_AGENT_PROFILE,
    ])
})

test('ignores invalid and duplicate stored profiles while preserving the default', () => {
    const storage = memoryStorage(
        JSON.stringify([
            { id: 'custom-1', name: '  整合包助手  ', instructions: '帮助安装整合包。' },
            { id: 'custom-1', name: '重复项', instructions: '' },
            { id: 'invalid', name: '', instructions: '' },
            {
                id: DEFAULT_AI_AGENT_PROFILE.id,
                name: '我的 MC-Trin 助手',
                instructions: '自定义指令',
            },
        ]),
    )

    assert.deepEqual(getAgentProfiles(storage), [
        { ...DEFAULT_AI_AGENT_PROFILE, name: '我的 MC-Trin 助手', instructions: '自定义指令' },
        { id: 'custom-1', name: '整合包助手', instructions: '帮助安装整合包。' },
    ])
})

test('adds, updates, and removes custom profiles without deleting the default', () => {
    const storage = memoryStorage()
    const afterAdd = addAgentProfile(storage, {
        name: '模组助手',
        instructions: '说明模组兼容性。',
    })
    const added = afterAdd[1]
    assert.ok(added.id)

    const afterUpdate = updateAgentProfile(storage, added.id, {
        instructions: '协助诊断模组冲突。',
    })
    assert.equal(afterUpdate[1].instructions, '协助诊断模组冲突。')

    assert.deepEqual(removeAgentProfile(storage, DEFAULT_AI_AGENT_PROFILE.id), afterUpdate)
    assert.deepEqual(removeAgentProfile(storage, added.id), [DEFAULT_AI_AGENT_PROFILE])
    assert.deepEqual(getAgentProfiles(storage), [DEFAULT_AI_AGENT_PROFILE])
})

test('enforces name and instruction length limits', () => {
    const storage = memoryStorage()
    assert.equal(
        addAgentProfile(storage, { name: 'a'.repeat(40), instructions: 'x'.repeat(2000) }).length,
        2,
    )
    assert.throws(
        () => addAgentProfile(storage, { name: 'a'.repeat(41), instructions: '' }),
        TypeError,
    )
    assert.throws(
        () => addAgentProfile(storage, { name: '有效名称', instructions: 'x'.repeat(2001) }),
        TypeError,
    )
    assert.throws(
        () => updateAgentProfile(storage, DEFAULT_AI_AGENT_PROFILE.id, { name: '   ' }),
        TypeError,
    )
})
