import { onMounted, onScopeDispose } from 'vue'

export const INSTANCE_CONTENT_EVENT_DEBOUNCE_MS = 180

export interface InstanceContentEvent {
    event: string
    instance_id: string
    revision?: number
}

type RefreshBehaviour = 'bypass' | undefined
type EventListener = (event: InstanceContentEvent) => void
type TimerHandle = ReturnType<typeof setTimeout>

async function subscribeToInstanceEvents(listener: EventListener) {
    const { instance_listener } = await import('../helpers/events.js')
    return instance_listener(listener)
}

export interface InstanceContentEventsOptions {
    instanceId: () => string
    installStage: () => string | undefined
    snapshotRevision: () => number | null
    isBulkOperating: () => boolean
    refresh: (behaviour?: RefreshBehaviour) => void | Promise<void>
    invalidate: (clearState?: boolean) => void
    onError: (error: unknown) => void
    subscribe?: (listener: EventListener) => Promise<() => void>
    now?: () => number
    schedule?: (callback: () => void, delayMs: number) => TimerHandle
    cancel?: (timer: TimerHandle) => void
    debounceMs?: number
}

export function createInstanceContentEvents(options: InstanceContentEventsOptions) {
    const now = options.now ?? Date.now
    const schedule = options.schedule ?? setTimeout
    const cancel = options.cancel ?? clearTimeout
    const debounceMs = options.debounceMs ?? INSTANCE_CONTENT_EVENT_DEBOUNCE_MS

    let timer: TimerHandle | null = null
    let pendingExternalSync = false
    let pendingContentChangedRevision: number | null = null
    let pendingContentRefreshAfterBulk = false
    let suppressSyncedUntil = 0
    let unlisten: (() => void) | null = null
    let started = false
    let stopped = false

    function clearPendingRefresh() {
        if (timer !== null) {
            cancel(timer)
            timer = null
        }
        pendingExternalSync = false
        pendingContentChangedRevision = null
        pendingContentRefreshAfterBulk = false
    }

    function requestRefresh(behaviour?: RefreshBehaviour) {
        try {
            void Promise.resolve(options.refresh(behaviour)).catch(options.onError)
        } catch (error) {
            options.onError(error)
        }
    }

    function flushPendingRefresh() {
        timer = null
        if (stopped) return

        const refreshFilesystem = pendingExternalSync && pendingContentChangedRevision === null
        pendingExternalSync = false
        pendingContentChangedRevision = null
        if (options.isBulkOperating()) {
            pendingContentRefreshAfterBulk = true
            return
        }
        requestRefresh(refreshFilesystem ? 'bypass' : undefined)
    }

    function handleEvent(event: InstanceContentEvent) {
        if (stopped || event.instance_id !== options.instanceId()) return

        if (event.event === 'removed') {
            clearPendingRefresh()
            options.invalidate(true)
            return
        }

        if (options.installStage() !== 'installed') return
        if (event.event === 'synced') {
            if (now() < suppressSyncedUntil) return
            pendingExternalSync = true
        } else if (event.event === 'content_changed') {
            const snapshotRevision = options.snapshotRevision()
            if (
                event.revision != null &&
                snapshotRevision != null &&
                event.revision <= snapshotRevision
            ) {
                return
            }
            pendingContentChangedRevision = Math.max(
                pendingContentChangedRevision ?? 0,
                event.revision ?? 0,
            )
        } else {
            return
        }

        if (timer !== null) return
        timer = schedule(flushPendingRefresh, debounceMs)
    }

    function start() {
        if (started || stopped) return
        started = true
        const subscribe = options.subscribe ?? subscribeToInstanceEvents
        void subscribe(handleEvent)
            .then((stopListening) => {
                if (stopped) {
                    stopListening()
                    return
                }
                unlisten = stopListening
            })
            .catch((error: unknown) => {
                if (!stopped) options.onError(error)
            })
    }

    function stop() {
        if (stopped) return
        stopped = true
        clearPendingRefresh()
        unlisten?.()
        unlisten = null
    }

    return {
        handleEvent,
        refreshAfterBulk() {
            if (stopped || !pendingContentRefreshAfterBulk) return
            clearPendingRefresh()
            requestRefresh()
        },
        start,
        stop,
        suppressSyncedFor(durationMs: number) {
            suppressSyncedUntil = now() + durationMs
        },
    }
}

export function useInstanceContentEvents(options: InstanceContentEventsOptions) {
    const controller = createInstanceContentEvents(options)
    onMounted(controller.start)
    onScopeDispose(controller.stop)
    return controller
}
