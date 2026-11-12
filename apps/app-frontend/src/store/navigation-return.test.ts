import assert from 'node:assert/strict'
import test from 'node:test'

import { reactive } from 'vue'

import {
    clearBrowseReturnSnapshot,
    clearUpgradeFlow,
    completeBrowseReturnNavigation,
    consumeBrowseReturnSnapshot,
    consumeUpgradeFlow,
    hasBrowseReturnSnapshot,
    isBrowseReturnNavigation,
    isBrowseReturnSourcePath,
    parkUpgradeFlow,
    peekUpgradeFlow,
    prepareBrowseReturnNavigation,
    restoreUpgradeFlow,
    saveBrowseReturnSnapshot,
    upgradeProjectPath,
} from './navigation-return.ts'

function upgradeSnapshot(overrides: Record<string, unknown> = {}) {
    return {
        instanceId: 'instance-a',
        returnFullPath: '/instance/instance-a/upgrade/compatibility',
        targetEnvironment: null,
        plan: null,
        createFullBackup: true,
        sharedUpgradeMode: null,
        activeJobId: null,
        result: null,
        ...overrides,
    } as never
}

test('a browse snapshot is returned once and then cleared', () => {
    const url = '/browse/mod?m=100&o=100'
    saveBrowseReturnSnapshot({ url, scrollTop: 480, state: { currentPage: 2, hits: ['a'] } })
    assert.equal(prepareBrowseReturnNavigation(url, '/project/sodium'), true)

    assert.deepEqual(consumeBrowseReturnSnapshot(url), {
        url,
        scrollTop: 480,
        state: { currentPage: 2, hits: ['a'] },
    })
    assert.equal(
        consumeBrowseReturnSnapshot(url),
        null,
        'a consumed snapshot must not be returned a second time',
    )
    assert.equal(hasBrowseReturnSnapshot(url), false)
})

test('a browse snapshot for another url is left untouched', () => {
    saveBrowseReturnSnapshot({ url: '/browse/mod?page=2', scrollTop: 10, state: {} })

    assert.equal(consumeBrowseReturnSnapshot('/browse/mod?page=3'), null)
    assert.equal(
        hasBrowseReturnSnapshot('/browse/mod?page=2'),
        true,
        'a mismatched consume must not discard the stored snapshot',
    )
    clearBrowseReturnSnapshot()
})

test('the return marker is set only for source routes and cleared on completion', () => {
    const url = '/browse/mod?source=modrinth'
    saveBrowseReturnSnapshot({ url, scrollTop: 480, state: {} })

    assert.equal(prepareBrowseReturnNavigation(url, '/library'), false)
    assert.equal(
        hasBrowseReturnSnapshot(url),
        false,
        'ordinary navigation drops the snapshot instead of arming a return',
    )
    assert.equal(isBrowseReturnNavigation(url), false)

    saveBrowseReturnSnapshot({ url, scrollTop: 480, state: {} })
    assert.equal(prepareBrowseReturnNavigation(url, '/instance/example'), true)
    assert.equal(isBrowseReturnNavigation(url), true)
    completeBrowseReturnNavigation(url)
    assert.equal(isBrowseReturnNavigation(url), false)
})

test('only project, download and instance paths are browse return sources', () => {
    assert.equal(isBrowseReturnSourcePath('/project/sodium'), true)
    assert.equal(isBrowseReturnSourcePath('/project/sodium/versions'), true)
    assert.equal(isBrowseReturnSourcePath('/downloads'), true)
    assert.equal(isBrowseReturnSourcePath('/instance/example'), true)
    assert.equal(isBrowseReturnSourcePath('/library'), false)
})

test('consuming a browse snapshot that does not exist is a no-op', () => {
    clearBrowseReturnSnapshot()
    assert.equal(consumeBrowseReturnSnapshot('/browse/mod'), null)
    assert.equal(prepareBrowseReturnNavigation('/browse/mod', '/project/sodium'), false)
    assert.equal(
        isBrowseReturnNavigation('/browse/mod'),
        false,
        'a failed consume must not arm the return marker',
    )
})

test('a parked upgrade is one-shot and instance scoped', () => {
    clearUpgradeFlow()
    const snapshot = upgradeSnapshot()
    parkUpgradeFlow(snapshot)

    assert.equal(
        consumeUpgradeFlow('instance-b', snapshot.returnFullPath),
        null,
        'another instance must not take the parked flow',
    )
    assert.deepEqual(consumeUpgradeFlow('instance-a', snapshot.returnFullPath), snapshot)
    assert.equal(
        consumeUpgradeFlow('instance-a', snapshot.returnFullPath),
        null,
        'the parked flow is consumed exactly once',
    )
})

test('consuming a parked upgrade with the wrong return path is refused', () => {
    clearUpgradeFlow()
    parkUpgradeFlow(upgradeSnapshot())

    assert.equal(consumeUpgradeFlow('instance-a', '/somewhere/else'), null)
    assert.notEqual(
        peekUpgradeFlow('instance-a'),
        null,
        'a rejected consume must leave the parked flow available',
    )
    clearUpgradeFlow()
})

for (const route of ['compatibility', 'customize', 'confirm']) {
    test(`${route} restores the parked plan before consuming it`, () => {
        clearUpgradeFlow()
        const snapshot = upgradeSnapshot({
            returnFullPath: `/instance/instance-a/upgrade/${route}`,
            targetEnvironment: { gameVersion: '26.1.2' },
            plan: { id: 'same-plan' },
        })
        parkUpgradeFlow(snapshot)

        let hydratedPlanId: string | undefined
        const restored = restoreUpgradeFlow('instance-a', snapshot.returnFullPath, (value) => {
            hydratedPlanId = (value as { plan?: { id?: string } }).plan?.id
        })

        assert.equal(hydratedPlanId, 'same-plan', 'hydration runs before consumption')
        assert.equal(
            (restored as { plan?: { id?: string } } | null)?.plan?.id,
            'same-plan',
        )
        assert.equal(
            peekUpgradeFlow('instance-a'),
            null,
            'restoring consumes the parked flow',
        )
    })
}

test('restoring the confirm step keeps plan and confirm choices intact', () => {
    clearUpgradeFlow()
    const snapshot = upgradeSnapshot({
        returnFullPath: '/instance/instance-a/upgrade/confirm',
        targetEnvironment: { gameVersion: '26.1.2' },
        plan: {
            id: 'same-plan',
            selectedSolution: { kind: 'custom' },
            customConstraints: [{ contentId: 'root', versionId: 'fixed' }],
        },
        createFullBackup: false,
        sharedUpgradeMode: 'direct',
    })
    parkUpgradeFlow(snapshot)

    let restored: Record<string, unknown> | null = null
    restoreUpgradeFlow('instance-a', snapshot.returnFullPath, (value) => {
        restored = value as unknown as Record<string, unknown>
    })

    const plan = restored?.plan as {
        id: string
        selectedSolution: unknown
        customConstraints: unknown
    }
    assert.equal(plan.id, 'same-plan')
    assert.deepEqual(
        restored?.targetEnvironment,
        (snapshot as unknown as Record<string, unknown>).targetEnvironment,
    )
    assert.deepEqual(plan.selectedSolution, { kind: 'custom' })
    assert.deepEqual(plan.customConstraints, [{ contentId: 'root', versionId: 'fixed' }])
    assert.equal(restored?.createFullBackup, false)
    assert.equal(restored?.sharedUpgradeMode, 'direct')
})

test('a failed hydration leaves the parked snapshot available', () => {
    clearUpgradeFlow()
    const snapshot = upgradeSnapshot()
    parkUpgradeFlow(snapshot)

    assert.throws(() =>
        restoreUpgradeFlow('instance-a', snapshot.returnFullPath, () => {
            throw new Error('hydrate failed')
        }),
    )
    assert.deepEqual(
        peekUpgradeFlow('instance-a'),
        snapshot,
        'the flow must survive so the user can retry',
    )
    clearUpgradeFlow()
})

test('a parked reactive upgrade dto is detached from later mutations', () => {
    clearUpgradeFlow()
    const snapshot = reactive(upgradeSnapshot({ instanceId: 'reactive-instance' }) as never)
    parkUpgradeFlow(snapshot as never)
    ;(snapshot as unknown as { createFullBackup: boolean }).createFullBackup = false

    assert.equal(
        (peekUpgradeFlow('reactive-instance') as unknown as { createFullBackup: boolean })
            .createFullBackup,
        true,
        'the store must snapshot the value, not keep a live reference',
    )
    assert.equal(
        consumeUpgradeFlow('wrong-instance', '/instance/reactive-instance/upgrade/compatibility'),
        null,
    )
    clearUpgradeFlow()
})

test('upgrade project paths round-trip through the trusted providers', () => {
    assert.equal(upgradeProjectPath('modrinth', 'P7dR8mSH'), '/project/P7dR8mSH')
    assert.equal(upgradeProjectPath('curseforge', '123'), '/project/curseforge/123')
    assert.equal(upgradeProjectPath('local', 'pack'), null)
    assert.equal(upgradeProjectPath(null, 'unidentified'), null)
    assert.equal(
        upgradeProjectPath('modrinth', 'a/b c'),
        '/project/a%2Fb%20c',
        'the project id is encoded so it cannot add path segments',
    )
    assert.equal(upgradeProjectPath('modrinth', ''), null)
})

test('peeking and consuming an empty upgrade state is safe', () => {
    clearUpgradeFlow()
    assert.equal(peekUpgradeFlow(), null)
    assert.equal(peekUpgradeFlow('instance-a'), null)
    assert.equal(consumeUpgradeFlow('instance-a', '/anywhere'), null)
    assert.equal(
        restoreUpgradeFlow('instance-a', '/anywhere', () => {
            throw new Error('hydration must not run without a parked flow')
        }),
        null,
        'restoring nothing must not call hydrate',
    )
})
