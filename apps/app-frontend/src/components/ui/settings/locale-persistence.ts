/**
 * Orders a language switch: load the catalogue, store the preference, and only
 * then activate the locale.
 *
 * The order is the point. The Settings page remounts its category bodies when
 * the active locale changes, so activating first destroys the component that
 * owns the save and mounts a replacement whose `settings_get` SELECT reaches the
 * backend before the `settings_patch` write. The picker then paints the row the
 * switch is replacing while the page renders the new language, and a failed
 * write reports its error to a component that no longer exists. Storing first
 * and flipping last means every observer of the new locale, including the
 * remounted picker and the read it issues, sees the value that was saved.
 *
 * `prepare` and `activate` are separate so a catalogue that cannot be loaded
 * aborts the switch while nothing has been stored; activating afterwards only
 * flips a bundle that is already in memory. Every write therefore happens while
 * the component that requested it is still mounted.
 *
 * Only the newest run may commit. Two picks overlap whenever the second starts
 * before the first write resolves, so a stale run must neither write, activate,
 * nor report an outcome over the newer one. Writes are queued for the same
 * reason: dispatching them in parallel would let the backend apply the earlier
 * pick after the later one and leave the database behind the rendered language.
 */
export interface LocalePersistencePorts {
    /** Resolves the requested tag and loads its catalogue without activating it. */
    prepare(requested: string): Promise<string>
    /** Merges the locale into the stored settings. Resolves after the backend write. */
    write(locale: string): Promise<void>
    /** Activates a prepared catalogue and returns the locale that is active afterwards. */
    activate(locale: string): Promise<string>
}

export type LocalePersistenceOutcome =
    | { status: 'saved'; locale: string }
    | { status: 'failed'; error: unknown }
    /** A newer run took over; this one leaves the UI, the status and the flags to it. */
    | { status: 'superseded' }

export interface LocalePersistence {
    persist(requested: string): Promise<LocalePersistenceOutcome>
}

export interface LocaleOrderingState {
    current: number
    writes: Promise<unknown>
}

export function createLocaleOrderingState(): LocaleOrderingState {
    return { current: 0, writes: Promise.resolve() }
}

/**
 * Defaults to one state for the whole module rather than one per component
 * instance. The remount that follows a successful switch destroys the instance
 * that started the run, so a counter owned by the instance could not stop a run
 * that outlived it.
 */
const sharedOrdering = createLocaleOrderingState()

export function createLocalePersistence(
    ports: LocalePersistencePorts,
    ordering: LocaleOrderingState = sharedOrdering,
): LocalePersistence {
    function writeInOrder(locale: string, isCurrent: () => boolean): Promise<boolean> {
        const write = ordering.writes.then(async () => {
            // Re-checked at dispatch: a run that was overtaken while it queued
            // behind an earlier write must not reach the backend at all.
            if (!isCurrent()) return false
            await ports.write(locale)
            return true
        })
        ordering.writes = write.catch(() => undefined)
        return write
    }

    return {
        async persist(requested) {
            const run = ++ordering.current
            const isCurrent = () => run === ordering.current

            try {
                const prepared = await ports.prepare(requested)
                if (!isCurrent()) return { status: 'superseded' }
                if (!(await writeInOrder(prepared, isCurrent))) return { status: 'superseded' }
                if (!isCurrent()) return { status: 'superseded' }

                // Flipped only after the write landed, so the remount that the flip
                // triggers reads the stored value instead of the one it replaces.
                const locale = await ports.activate(prepared)

                // A different locale than the prepared one means the shared loader
                // dropped this activation because a newer selection overtook it.
                // That selection owns the state now, and writing after the flip
                // could only fail on an instance the flip already destroyed.
                if (locale !== prepared || !isCurrent()) return { status: 'superseded' }

                return { status: 'saved', locale }
            } catch (error) {
                if (!isCurrent()) return { status: 'superseded' }
                return { status: 'failed', error }
            }
        },
    }
}
