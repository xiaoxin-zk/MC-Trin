import assert from 'node:assert/strict'
import test from 'node:test'

import { moduleFilter, selectPlan, targetedCargoArgs } from './ci-targeted-tests.mjs'

test('maps Theseus source files to their module paths', () => {
    assert.equal(moduleFilter('packages/app-lib/src/state/friends.rs'), 'state::friends')
    assert.equal(moduleFilter('packages/app-lib/src/util/fetch/tests.rs'), 'util::fetch')
    assert.equal(moduleFilter('packages/app-lib/src/state/mod.rs'), 'state')
    assert.equal(moduleFilter('packages/app-lib/src/api/mctier/tests.rs'), 'api::mctier')
    assert.equal(moduleFilter('packages/app-lib/src/api/ai/catalog.rs'), 'api::ai')
    assert.equal(moduleFilter('packages/app-lib/src/util/fetch/connection_budget.rs'), 'util::fetch')
    assert.equal(
        moduleFilter(
            'packages/app-lib/src/state/instances/commands/instance_upgrade/solver.rs',
        ),
        'state::instances::commands::instance_upgrade',
    )
    assert.equal(
        moduleFilter(
            'packages/app-lib/src/state/instances/commands/apply_content_install/resolve.rs',
        ),
        'state::instances::commands::apply_content_install',
    )
    assert.equal(moduleFilter('packages/app-lib/src/lib.rs'), null)
})

test('targets the changed workspace crate and its tests', () => {
    const plan = selectPlan([
        'packages/app-lib/src/state/friends.rs',
        'packages/app-lib/src/api/mctier/protocol.rs',
        'apps/app/src/main.rs',
    ])
    assert.equal(plan.reason, 'changed Rust crates')
    assert.deepEqual(
        plan.crates.map(({ package: packageName, filters }) => [packageName, filters]),
        [
            ['theseus', ['api::mctier::protocol', 'state::friends']],
            ['theseus_gui', []],
        ],
    )
    const commands = targetedCargoArgs(plan)
    assert.deepEqual(commands[0], [
        'test',
        '--package',
        'theseus',
        '--lib',
        '--no-fail-fast',
        'api::mctier::protocol',
        '--',
        '--test-threads=1',
    ])
    assert(commands.some((args) => args.includes('theseus_gui')))
})

test('runs the whole crate when its root, build script or manifest changes', () => {
    for (const file of [
        'packages/app-lib/src/lib.rs',
        'packages/app-lib/build.rs',
        'packages/app-lib/Cargo.toml',
    ]) {
        const plan = selectPlan([file])
        assert.equal(plan.crates.length, 1, file)
        assert.equal(plan.crates[0].package, 'theseus', file)
        assert.deepEqual(plan.crates[0].filters, [], file)
        assert.equal(targetedCargoArgs(plan)[0][0], 'test', file)
    }
})

test('runs the standalone Minecraft ping crate through its manifest', () => {
    const args = targetedCargoArgs(selectPlan(['packages/async-minecraft-ping/src/lib.rs']))[0]
    assert.deepEqual(args.slice(0, 3), [
        'test',
        '--manifest-path',
        'packages/async-minecraft-ping/Cargo.toml',
    ])
})

test('checks all workspace crates separately for workspace-wide changes', () => {
    const plan = selectPlan(['Cargo.lock'])
    assert.equal(plan.reason, 'workspace-wide change')
    const commands = targetedCargoArgs(plan)
    assert(commands.some((args) => args[0] === 'check' && args.includes('theseus')))
    assert(commands.some((args) => args[0] === 'check' && args.includes('theseus_gui')))
    assert(commands.some((args) => args[0] === 'check' && args.includes('axolotl-installer-ui')))
    assert(commands.some((args) => args[0] === 'check' && args.includes('content-management')))
})

test('unknown Rust workspace paths fall back to all known crates', () => {
    const plan = selectPlan(['packages/new-crate/src/lib.rs'])
    assert.equal(plan.reason, 'unmapped Rust crate')
    assert(targetedCargoArgs(plan).every((args) => args[0] === 'check'))
})

test('a missing baseline compiles all workspace targets instead of testing everything', () => {
    const plan = selectPlan(null)
    assert.equal(plan.reason, 'no baseline')
    assert(targetedCargoArgs(plan).every((args) => args[0] === 'check'))
})

test('does not run Rust tests for frontend-only changes', () => {
    const plan = selectPlan(['apps/app-frontend/src/App.vue', 'README.md'])
    assert.equal(plan.reason, 'no Rust change')
    assert.deepEqual(targetedCargoArgs(plan), [])
})

test('deduplicates child filters when a parent module is selected', () => {
    const filters = selectPlan([
        'packages/app-lib/src/api/mod.rs',
        'packages/app-lib/src/api/mctier.rs',
    ]).crates[0].filters
    assert.deepEqual(filters, ['api'])
})
