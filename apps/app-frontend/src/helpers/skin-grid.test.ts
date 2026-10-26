import assert from 'node:assert/strict'
import { test } from 'node:test'

import { skinColumnCountForWidth } from './skin-grid.ts'

test('skin columns follow the measured split-pane width', () => {
    assert.equal(skinColumnCountForWidth(0), 3)
    assert.equal(skinColumnCountForWidth(320), 1)
    assert.equal(skinColumnCountForWidth(700), 4)
    assert.equal(skinColumnCountForWidth(1200), 6)
    assert.equal(skinColumnCountForWidth(Number.NaN), 3)
})
