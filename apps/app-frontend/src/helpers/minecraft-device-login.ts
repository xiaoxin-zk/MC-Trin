export interface MinecraftDeviceLoginFlow {
    device_code: string
    user_code: string
    verification_uri: string
    expires_in: number
    interval: number
}

export type MinecraftDeviceLoginPoll<T> =
    { status: 'pending'; slow_down: boolean } | { status: 'complete'; credentials: T }

export interface MinecraftDeviceLoginState {
    busy: boolean
    flow: MinecraftDeviceLoginFlow | null
    error: string | null
}

interface Dependencies<T> {
    begin: () => Promise<MinecraftDeviceLoginFlow>
    poll: (deviceCode: string) => Promise<MinecraftDeviceLoginPoll<T>>
    change: (state: MinecraftDeviceLoginState) => void
    complete: (credentials: T) => void
    expiredMessage: () => string
    now?: () => number
    schedule?: (callback: () => void, delay: number) => () => void
}

export function minecraftLoginErrorMessage(error: unknown): string {
    if (error instanceof Error) return error.message
    if (typeof error === 'string') return error
    if (error && typeof error === 'object' && 'message' in error) return String(error.message)
    return JSON.stringify(error) ?? String(error)
}

/** Owns one device flow; closing or retrying invalidates every in-flight response. */
export function createMinecraftDeviceLogin<T>(dependencies: Dependencies<T>) {
    const now = dependencies.now ?? Date.now
    const schedule =
        dependencies.schedule ??
        ((callback: () => void, delay: number) => {
            const timer = setTimeout(callback, delay)
            return () => clearTimeout(timer)
        })
    let generation = 0
    let cancelTimer: (() => void) | undefined
    let state: MinecraftDeviceLoginState = { busy: false, flow: null, error: null }

    function publish(update: Partial<MinecraftDeviceLoginState>) {
        state = { ...state, ...update }
        dependencies.change(state)
    }

    function stop() {
        generation++
        cancelTimer?.()
        cancelTimer = undefined
        publish({ busy: false, flow: null, error: null })
    }

    function waitForPoll(
        current: number,
        flow: MinecraftDeviceLoginFlow,
        expiresAt: number,
        delay: number,
    ) {
        const remaining = expiresAt - now()
        if (remaining <= 0) {
            publish({ error: dependencies.expiredMessage() })
            return
        }
        cancelTimer = schedule(
            () => {
                cancelTimer = undefined
                void poll(current, flow, expiresAt, delay)
            },
            Math.min(delay, remaining),
        )
    }

    async function poll(
        current: number,
        flow: MinecraftDeviceLoginFlow,
        expiresAt: number,
        delay: number,
    ) {
        if (current !== generation) return
        if (now() >= expiresAt) {
            publish({ error: dependencies.expiredMessage() })
            return
        }
        try {
            const result = await dependencies.poll(flow.device_code)
            if (current !== generation) return
            if (result.status === 'complete') {
                stop()
                dependencies.complete(result.credentials)
                return
            }
            waitForPoll(current, flow, expiresAt, result.slow_down ? delay + 5000 : delay)
        } catch (error) {
            if (current === generation) publish({ error: minecraftLoginErrorMessage(error) })
        }
    }

    async function start() {
        stop()
        const current = generation
        publish({ busy: true })
        try {
            const flow = await dependencies.begin()
            if (current !== generation) return
            if (!(flow.expires_in > 0)) {
                publish({ error: dependencies.expiredMessage() })
                return
            }
            const delay = (flow.interval > 0 ? flow.interval : 5) * 1000
            publish({ flow })
            // RFC 8628 requires waiting at least interval before each token request.
            waitForPoll(current, flow, now() + flow.expires_in * 1000, delay)
        } catch (error) {
            if (current === generation) publish({ error: minecraftLoginErrorMessage(error) })
        } finally {
            if (current === generation) publish({ busy: false })
        }
    }

    return { start, stop }
}
