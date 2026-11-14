import { computed, type ComputedRef, type Ref, ref } from 'vue'

import type { RemoteMcpTool } from './types.ts'

/** One tool call waiting for the user to allow or deny it. */
export type PendingApproval = {
    endpoint: string
    tool: RemoteMcpTool
    args: Record<string, unknown>
    /** Settles the caller and removes the request from the queue. */
    resolve: (approved: boolean) => void
    timer?: ReturnType<typeof setTimeout>
}

export type ApprovalQueue = {
    pending: Readonly<Ref<PendingApproval[]>>
    current: ComputedRef<PendingApproval | null>
    request(endpoint: string, tool: RemoteMcpTool, args: Record<string, unknown>): Promise<boolean>
    /** Answers the dialog currently on screen. */
    resolve(approved: boolean): void
    /** Denies everything still waiting, e.g. when the run is cancelled. */
    denyAll(): void
}

type Entry = PendingApproval & { id: number; settled: boolean }

/**
 * A queue of approval dialogs where every entry has a deadline.
 *
 * The dialog is modal, so only the head is displayed; a request nobody answers
 * used to stay pending until the component unmounted, which left the agent turn
 * waiting on a promise nothing could settle. Each entry now expires after
 * `timeoutMs`, settles as denied, leaves the queue and reports the tool through
 * `onTimeout`, so the run always continues or fails visibly.
 *
 * Entries carry their own id and settled flag because the queue is a Vue `ref`:
 * reading it returns reactive proxies, and comparing them with the raw object a
 * timer closed over would never match.
 */
export function createApprovalQueue(
    timeoutMs: number,
    onTimeout: (tool: RemoteMcpTool) => void,
): ApprovalQueue {
    const entries = ref<Entry[]>([])
    const pending = computed<Entry[]>(() => entries.value)
    const current = computed(() => pending.value[0] ?? null)
    let nextId = 0

    function settle(entry: Entry, approved: boolean): void {
        if (entry.settled) return
        entry.settled = true
        if (entry.timer !== undefined) clearTimeout(entry.timer)
        entry.timer = undefined
        entries.value = entries.value.filter((candidate) => candidate.id !== entry.id)
        entry.resolve(approved)
    }

    function request(
        endpoint: string,
        tool: RemoteMcpTool,
        args: Record<string, unknown>,
    ): Promise<boolean> {
        return new Promise<boolean>((resolve) => {
            const id = nextId++
            const entry: Entry = {
                id,
                settled: false,
                endpoint,
                tool,
                args,
                resolve,
            }
            entry.timer = setTimeout(() => {
                settle(entry, false)
                onTimeout(tool)
            }, timeoutMs)
            entries.value = [...entries.value, entry]
        })
    }

    function resolve(approved: boolean): void {
        const entry = entries.value[0]
        if (entry) settle(entry, approved)
    }

    function denyAll(): void {
        for (const entry of [...entries.value]) settle(entry, false)
    }

    return { pending, current, request, resolve, denyAll }
}
