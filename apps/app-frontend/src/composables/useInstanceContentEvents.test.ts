import assert from 'node:assert/strict'
import test from 'node:test'

import {
    createInstanceContentEvents,
    type InstanceContentEvent,
} from './useInstanceContentEvents.ts'

function fixture(options: { bulk?: boolean; revision?: number | null } = {}) {
    let currentTime = 100
    let nextTimerId = 0
    const timers = new Map<number, () => void>()
    const refreshes: Array<'bypass' | undefined> = []
    let invalidations = 0
    let bulk = options.bulk ?? false
    let revision = options.revision ?? 1

    const controller = createInstanceContentEvents({
        instanceId: () => 'instance-1',
        installStage: () => 'installed',
        snapshotRevision: () => revision,
        isBulkOperating: () => bulk,
        refresh: (behaviour) => refreshes.push(behaviour),
        invalidate: (clearState) => {
            if (clearState) invalidations += 1
        },
        onError: (error) => {
            throw error
        },
        now: () => currentTime,
        schedule: (callback) => {
            const id = nextTimerId++
            timers.set(id, callback)
            return id as unknown as ReturnType<typeof setTimeout>
        },
        cancel: (timer) => timers.delete(timer as unknown as number),
        debounceMs: 180,
    })

    return {
        controller,
        fireTimers() {
            const pending = [...timers.values()]
            timers.clear()
            pending.forEach((callback) => callback())
        },
        setBulk(value: boolean) {
            bulk = value
        },
        setRevision(value: number | null) {
            revision = value
        },
        setTime(value: number) {
            currentTime = value
        },
        timers,
        refreshes,
        invalidations: () => invalidations,
    }
}

function event(event: string, revision?: number): InstanceContentEvent {
    return { event, instance_id: 'instance-1', revision }
}

test('ignores self-generated syncs and stale content revisions', () => {
    const state = fixture({ revision: 4 })
    state.controller.suppressSyncedFor(500)

    state.controller.handleEvent(event('synced'))
    state.controller.handleEvent(event('content_changed', 4))
    state.fireTimers()

    assert.equal(state.timers.size, 0)
    assert.deepEqual(state.refreshes, [])

    state.setTime(601)
    state.controller.handleEvent(event('synced'))
    state.fireTimers()

    assert.deepEqual(state.refreshes, ['bypass'])
})

test('defers a debounced event refresh until bulk work finishes', () => {
    const state = fixture({ bulk: true, revision: 2 })

    state.controller.handleEvent(event('content_changed', 3))
    state.fireTimers()
    assert.deepEqual(state.refreshes, [])
    state.controller.handleEvent(event('content_changed', 4))

    state.setBulk(false)
    state.controller.refreshAfterBulk()
    state.fireTimers()
    state.controller.refreshAfterBulk()
    assert.deepEqual(state.refreshes, [undefined])
    assert.equal(state.timers.size, 0)
})

test('unlistens and cancels a pending debounce when stopped', async () => {
    let listener: ((event: InstanceContentEvent) => void) | undefined
    let unlistened = false
    const state = fixture()
    const controller = createInstanceContentEvents({
        instanceId: () => 'instance-1',
        installStage: () => 'installed',
        snapshotRevision: () => 1,
        isBulkOperating: () => false,
        refresh: (behaviour) => state.refreshes.push(behaviour),
        invalidate: () => {},
        onError: (error) => {
            throw error
        },
        subscribe: async (callback) => {
            listener = callback
            return () => {
                unlistened = true
            }
        },
        schedule: (callback) => {
            state.timers.set(0, callback)
            return 0 as unknown as ReturnType<typeof setTimeout>
        },
        cancel: (timer) => state.timers.delete(timer as unknown as number),
    })

    controller.start()
    await Promise.resolve()
    listener?.(event('content_changed', 2))
    assert.equal(state.timers.size, 1)

    controller.stop()
    state.fireTimers()
    listener?.(event('content_changed', 3))

    assert.equal(unlistened, true)
    assert.deepEqual(state.refreshes, [])
    assert.equal(state.timers.size, 0)
})

test('removed instances invalidate state and discard pending refreshes', () => {
    const state = fixture()
    state.controller.handleEvent(event('content_changed', 2))
    state.controller.handleEvent(event('removed'))
    state.fireTimers()

    assert.equal(state.invalidations(), 1)
    assert.deepEqual(state.refreshes, [])
    assert.equal(state.timers.size, 0)
})
