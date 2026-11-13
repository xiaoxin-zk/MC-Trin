import assert from 'node:assert/strict'
import test from 'node:test'

import {
    branchedTargets,
    configuredTargets,
    missingTargets,
    renderedTargets,
} from './check-onboarding-targets.mjs'

test('collects the targets a tour configuration points at', () => {
    const config = `
        const steps = [
            step('a', 'inspect', copy(x, y, z), { targetId: 'nav-library' }),
            step('b', 'inspect', copy(x, y, z), { targetId: 'instance-tabs' }),
        ]
    `
    assert.deepEqual([...configuredTargets(config)].sort(), ['instance-tabs', 'nav-library'])
})

test('collects targets referenced through branchByTarget', () => {
    const config = `
        step('a', 'navigate', copy(x, y, z), {
            targetId: 'create-instance',
            branchByTarget: {
                'creation-method-custom': 'b',
                'creation-method-import': 'c',
            },
        })
    `
    assert.deepEqual(
        [...branchedTargets(config)].sort(),
        ['creation-method-custom', 'creation-method-import'],
    )
})

test('collects the targets a template renders', () => {
    const template = `
        <nav data-onboarding-id="nav-library" />
        <div data-onboarding-id="instance-tabs" />
    `
    assert.deepEqual([...renderedTargets(template)].sort(), ['instance-tabs', 'nav-library'])
})

test('reports a configured target that no component renders', () => {
    const configured = new Set(['nav-library', 'mctier-visibility'])
    const rendered = new Set(['nav-library'])
    assert.deepEqual(missingTargets(configured, rendered), ['mctier-visibility'])
})

test('reports nothing when every target resolves', () => {
    const targets = new Set(['nav-library', 'mctier-visibility'])
    assert.deepEqual(missingTargets(targets, new Set(targets)), [])
})

test('lets two tours share one target without a false failure', () => {
    const configured = new Set(['assistant-permission', 'assistant-permission'])
    assert.deepEqual(missingTargets(configured, new Set(['assistant-permission'])), [])
})
