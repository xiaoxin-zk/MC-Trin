import assert from 'node:assert/strict'
import test from 'node:test'

import {
    moduleFilter,
    selectFilters,
    targetedCargoArgs,
} from './ci-targeted-tests.mjs'

test('maps a source file to its module path', () => {
    assert.equal(
        moduleFilter('packages/app-lib/src/state/friends.rs'),
        'state::friends',
    )
    assert.equal(
        moduleFilter('packages/app-lib/src/util/fetch/tests.rs'),
        'util::fetch',
    )
    assert.equal(moduleFilter('packages/app-lib/src/state/mod.rs'), 'state')
    assert.equal(
        moduleFilter('packages/app-lib/src/api/mctier/tests.rs'),
        'api::mctier',
    )
})

test('ignores files that are not crate sources', () => {
    assert.equal(moduleFilter('apps/app-frontend/src/App.vue'), null)
    assert.equal(moduleFilter('packages/app-lib/src/lib.rs'), 'lib')
    assert.equal(moduleFilter('README.md'), null)
})

test('selects one filter per changed module', () => {
    const { filters, reason } = selectFilters([
        'packages/app-lib/src/state/friends.rs',
        'packages/app-lib/src/state/friends.rs',
        'packages/app-lib/src/api/mctier/protocol.rs',
        'apps/app-frontend/src/App.vue',
    ])
    assert.equal(reason, 'changed modules')
    assert.deepEqual(filters, ['api::mctier::protocol', 'state::friends'])
})

test('runs nothing when no crate source changed', () => {
    const { filters, reason } = selectFilters(['apps/website/app.vue'])
    assert.equal(reason, 'no Rust change')
    assert.deepEqual(targetedCargoArgs(filters), null)
})

test('falls back to the whole suite for crate-wide changes', () => {
    const { filters, reason } = selectFilters(['Cargo.lock'])
    assert.equal(reason, 'crate-wide change')
    assert.equal(filters, null)
    assert.deepEqual(targetedCargoArgs(filters), [
        'test',
        '--package',
        'theseus',
        '--lib',
        '--no-fail-fast',
        '--',
        '--test-threads=1',
    ])
})

test('emits one cargo command per module filter', () => {
    const commands = targetedCargoArgs(['state::friends', 'api::mctier'])
    assert.equal(commands.length, 2)
    assert.deepEqual(commands[0], [
        'test',
        '--package',
        'theseus',
        '--lib',
        '--no-fail-fast',
        'state::friends',
        '--',
        '--test-threads=1',
    ])
})
