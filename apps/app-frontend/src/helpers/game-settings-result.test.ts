import assert from 'node:assert/strict'
import test from 'node:test'

import type { SaveGameSettingsResult } from './game-options.ts'
import { hasIncompleteGameSettingsSave } from './game-settings-result.ts'

const complete: SaveGameSettingsResult = {
    applied: 2,
    migrated: 0,
    deferred: 0,
    unsupported: 0,
    failed: 0,
}

test('reports deferred, unsupported, and failed instance updates as incomplete', () => {
    assert.equal(hasIncompleteGameSettingsSave(complete), false)
    assert.equal(hasIncompleteGameSettingsSave({ ...complete, deferred: 1 }), true)
    assert.equal(hasIncompleteGameSettingsSave({ ...complete, unsupported: 1 }), true)
    assert.equal(hasIncompleteGameSettingsSave({ ...complete, failed: 1 }), true)
})
