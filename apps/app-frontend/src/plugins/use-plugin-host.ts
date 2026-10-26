import { defineMessages, injectNotificationManager, useVIntl } from '@modrinth/ui'
import { confirm } from '@tauri-apps/plugin-dialog'
import { useRouter } from 'vue-router'

import { list, run } from '@/helpers/instance'
import { update } from '@/helpers/settings'
import { useTheming } from '@/store/state'

import {
    PLUGIN_TARGETS,
    pluginAccentToLauncher,
    type PluginHost,
    type PluginInstance,
} from './local-plugin-runtime'

const messages = defineMessages({
    notice: { id: 'app.plugins.notice', defaultMessage: 'Plugin message' },
    launchTitle: { id: 'app.plugins.launch.title', defaultMessage: 'Plugin launch request' },
    launch: {
        id: 'app.plugins.launch.confirm',
        defaultMessage: 'Allow {plugin} to launch {instance}?',
    },
    allow: { id: 'app.plugins.launch.allow', defaultMessage: 'Launch instance' },
    deny: { id: 'app.plugins.launch.deny', defaultMessage: 'Cancel' },
    noInstance: {
        id: 'app.plugins.launch.no-instance',
        defaultMessage: 'Create an installed instance before launching it from a plugin.',
    },
})

/** No paths, native invoke function, secrets, or host DOM objects cross this boundary. */
export function usePluginHost(): PluginHost {
    const router = useRouter()
    const theme = useTheming()
    const { formatMessage } = useVIntl()
    const { addNotification } = injectNotificationManager()
    const instances = async () => {
        const items = await list()
        return [...items].sort((left, right) => {
            const date = (value: unknown) => (value ? new Date(String(value)).getTime() || 0 : 0)
            return date(right.last_played) - date(left.last_played)
        })
    }
    return {
        async instances(): Promise<PluginInstance[]> {
            const items = await instances()
            const selected = items.find((item) => item.install_stage === 'installed')?.id
            return items.map((item) => ({
                id: item.id,
                name: item.name,
                gameVersion: item.game_version,
                loader: item.loader,
                selected: item.id === selected,
            }))
        },
        toast(message) {
            addNotification({ type: 'info', title: formatMessage(messages.notice), text: message })
        },
        async theme(value) {
            const saved = await update({
                accent_color: pluginAccentToLauncher(value.accent),
                ...(value.mode ? { theme: value.mode } : {}),
            })
            theme.setAccentColor(saved.accent_color)
            if (value.mode) theme.setThemeState(saved.theme)
        },
        navigate: async (target) => router.push(PLUGIN_TARGETS[target]),
        async launchDefault(pluginName) {
            const item = (await instances()).find(
                (candidate) => candidate.install_stage === 'installed',
            )
            if (!item) throw new Error(formatMessage(messages.noInstance))
            const approved = await confirm(
                formatMessage(messages.launch, { plugin: pluginName, instance: item.name }),
                {
                    title: formatMessage(messages.launchTitle),
                    kind: 'warning',
                    okLabel: formatMessage(messages.allow),
                    cancelLabel: formatMessage(messages.deny),
                },
            )
            if (!approved) return false
            await run(item.id)
            return true
        },
    }
}
