import assert from 'node:assert/strict'
import test from 'node:test'

import {
    createMinecraftDeviceLogin,
    type MinecraftDeviceLoginFlow,
    type MinecraftDeviceLoginPoll,
    type MinecraftDeviceLoginState,
    minecraftLoginErrorMessage,
} from './minecraft-device-login.ts'

const flow: MinecraftDeviceLoginFlow = {
    device_code: 'private-device-code',
    user_code: 'ABCD-EFGH',
    verification_uri: 'https://microsoft.com/devicelogin',
    expires_in: 900,
    interval: 5,
}

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: Error) => void
    const promise = new Promise<T>((yes, no) => {
        resolve = yes
        reject = no
    })
    return { promise, resolve, reject }
}

function harness(
    options: {
        begin?: () => Promise<MinecraftDeviceLoginFlow>
        poll?: (deviceCode: string) => Promise<MinecraftDeviceLoginPoll<string>>
    } = {},
) {
    let time = 0
    let state!: MinecraftDeviceLoginState
    const completed: string[] = []
    const calls: { time: number; code: string }[] = []
    const timers = new Set<{ callback: () => void; deadline: number }>()
    const session = createMinecraftDeviceLogin<string>({
        begin: options.begin ?? (async () => flow),
        poll: async (code) => {
            calls.push({ time, code })
            return options.poll ? options.poll(code) : { status: 'pending', slow_down: false }
        },
        change: (next) => {
            state = next
        },
        complete: (credential) => {
            completed.push(credential)
        },
        expiredMessage: () => 'Code expired',
        now: () => time,
        schedule: (callback, delay) => {
            const timer = { callback, deadline: time + delay }
            timers.add(timer)
            return () => {
                timers.delete(timer)
            }
        },
    })
    const settle = async () => {
        for (let index = 0; index < 8; index++) await Promise.resolve()
    }
    async function tick(milliseconds: number) {
        const end = time + milliseconds
        for (;;) {
            const timer = [...timers]
                .filter((item) => item.deadline <= end)
                .sort((left, right) => left.deadline - right.deadline)[0]
            if (!timer) break
            timers.delete(timer)
            time = timer.deadline
            timer.callback()
            await settle()
        }
        time = end
    }
    return {
        ...session,
        tick,
        settle,
        get state() {
            return state
        },
        timers,
        completed,
        calls,
    }
}

test('waits for the Microsoft interval before the first and each subsequent token request', async () => {
    const login = harness()
    await login.start()
    assert.equal(login.state.flow, flow)
    assert.equal(login.state.busy, false)
    await login.tick(4999)
    assert.equal(login.calls.length, 0)
    await login.tick(1)
    assert.deepEqual(login.calls, [{ time: 5000, code: flow.device_code }])
    await login.tick(5000)
    assert.equal(login.calls[1].time, 10000)
    login.stop()
})

test('slow_down adds five seconds to this flow for every subsequent request', async () => {
    const login = harness({ poll: async () => ({ status: 'pending', slow_down: true }) })
    await login.start()
    await login.tick(5000)
    await login.tick(9999)
    assert.equal(login.calls.length, 1)
    await login.tick(1)
    await login.tick(15000)
    assert.deepEqual(
        login.calls.map((call) => call.time),
        [5000, 15000, 30000],
    )
    login.stop()
})

test('closing during the initial request discards a late code and starts no polling', async () => {
    const begin = deferred<MinecraftDeviceLoginFlow>()
    const login = harness({ begin: () => begin.promise })
    const pending = login.start()
    assert.equal(login.state.busy, true)
    login.stop()
    begin.resolve(flow)
    await pending
    await login.tick(20000)
    assert.deepEqual(login.state, { busy: false, flow: null, error: null })
    assert.equal(login.calls.length, 0)
})

test('closing during polling suppresses a late successful completion', async () => {
    const poll = deferred<MinecraftDeviceLoginPoll<string>>()
    const login = harness({ poll: () => poll.promise })
    await login.start()
    await login.tick(5000)
    login.stop()
    poll.resolve({ status: 'complete', credentials: 'stale-account' })
    await login.settle()
    assert.deepEqual(login.completed, [])
    assert.equal(login.timers.size, 0)
})

test('retrying while an old request fails cannot replace the new code or clear its busy state', async () => {
    const first = deferred<MinecraftDeviceLoginFlow>()
    const second = deferred<MinecraftDeviceLoginFlow>()
    let request = 0
    const login = harness({ begin: () => (++request === 1 ? first.promise : second.promise) })
    const old = login.start()
    const current = login.start()
    first.reject(new Error('Old failure'))
    await old
    assert.equal(login.state.busy, true)
    assert.equal(login.state.error, null)
    second.resolve({ ...flow, device_code: 'new-device-code' })
    await current
    await login.tick(5000)
    assert.equal(login.calls[0].code, 'new-device-code')
    login.stop()
})

test('expiry is shown at the deadline without a late token request', async () => {
    const login = harness({ begin: async () => ({ ...flow, expires_in: 2 }) })
    await login.start()
    await login.tick(2000)
    assert.equal(login.state.error, 'Code expired')
    assert.equal(login.calls.length, 0)
    assert.equal(login.timers.size, 0)
})

test('a failed token request can be retried with a fresh flow and finishes exactly once', async () => {
    let requests = 0
    const login = harness({
        poll: async () => {
            if (++requests === 1) throw new Error('Connection unavailable')
            return { status: 'complete', credentials: 'microsoft:player' }
        },
    })
    await login.start()
    await login.tick(5000)
    assert.equal(login.state.error, 'Connection unavailable')
    await login.start()
    assert.equal(login.state.error, null)
    await login.tick(30000)
    assert.deepEqual(login.completed, ['microsoft:player'])
    assert.equal(login.state.flow, null)
    assert.equal(login.timers.size, 0)
})

test('uses RFC 8628 default interval when the provider omits a positive interval', async () => {
    const login = harness({ begin: async () => ({ ...flow, interval: 0 }) })
    await login.start()
    await login.tick(4999)
    assert.equal(login.calls.length, 0)
    await login.tick(1)
    assert.equal(login.calls.length, 1)
    login.stop()
})

test('preserves meaningful native error messages', () => {
    assert.equal(minecraftLoginErrorMessage(new Error('Timed out')), 'Timed out')
    assert.equal(minecraftLoginErrorMessage({ message: 'Permission denied' }), 'Permission denied')
    assert.equal(
        minecraftLoginErrorMessage({ type: 'error', value: 'Network error' }),
        '{"type":"error","value":"Network error"}',
    )
})
