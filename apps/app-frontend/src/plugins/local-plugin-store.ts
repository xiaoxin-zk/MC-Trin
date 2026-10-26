import { shallowRef } from 'vue'

import {
    type InstalledLocalPlugin,
    LOCAL_PLUGINS_KEY,
    type LocalPluginManifest,
    readLocalPlugins,
    writeLocalPlugins,
} from './local-plugin-runtime'

const installed = shallowRef<InstalledLocalPlugin[]>([])
let loaded = false

export function useLocalPlugins() {
    if (!loaded) {
        loaded = true
        try {
            installed.value = readLocalPlugins(localStorage.getItem(LOCAL_PLUGINS_KEY))
        } catch {
            installed.value = []
        }
    }
    function persist(next: InstalledLocalPlugin[]) {
        const json = writeLocalPlugins(next)
        // Keep the prior state if the browser refuses persistence.
        localStorage.setItem(LOCAL_PLUGINS_KEY, json)
        installed.value = next
    }
    return {
        installed,
        install(manifest: LocalPluginManifest) {
            const next = installed.value.filter((plugin) => plugin.manifest.id !== manifest.id)
            persist([...next, { manifest, enabled: true }])
        },
        setEnabled(id: string, enabled: boolean) {
            persist(
                installed.value.map((plugin) =>
                    plugin.manifest.id === id ? { ...plugin, enabled } : plugin,
                ),
            )
        },
        remove(id: string) {
            persist(installed.value.filter((plugin) => plugin.manifest.id !== id))
        },
    }
}
