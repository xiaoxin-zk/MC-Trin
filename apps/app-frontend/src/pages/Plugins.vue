<script setup lang="ts">
import { PackageIcon, UploadIcon } from '@modrinth/assets'
import { Button, useVIntl } from '@modrinth/ui'
import { confirm } from '@tauri-apps/plugin-dialog'
import { computed, ref } from 'vue'

import PluginSandbox from '@/components/plugins/PluginSandbox.vue'
import {
    type InstalledLocalPlugin,
    type LocalPluginManifest,
    MAX_MANIFEST_BYTES,
    parseLocalPlugin,
    type PluginPermission,
    runLocalPluginAction,
} from '@/plugins/local-plugin-runtime'
import { useLocalPlugins } from '@/plugins/local-plugin-store'
import { exampleLocalPlugin, pluginMessages as messages } from '@/plugins/messages'
import { usePluginHost } from '@/plugins/use-plugin-host'

const { formatMessage } = useVIntl()
const host = usePluginHost()
const plugins = useLocalPlugins()
const picker = ref<HTMLInputElement>()
const activeId = ref('')
const pendingManifest = ref<LocalPluginManifest | null>(null)
const pendingEnable = ref(false)
const guideVisible = ref(false)
const error = ref('')
const notice = ref('')
const busy = ref(false)
const active = computed(() =>
    plugins.installed.value.find(
        (plugin) => plugin.enabled && plugin.manifest.id === activeId.value,
    ),
)
const replacing = computed(
    () =>
        pendingManifest.value &&
        plugins.installed.value.some((plugin) => plugin.manifest.id === pendingManifest.value!.id),
)
const permissionMessages = {
    'instances.read': messages.instancesPermission,
    'ui.toast': messages.toastPermission,
    'ui.theme': messages.themePermission,
    'app.navigate': messages.navigatePermission,
    'game.launchDefault': messages.launchPermission,
} satisfies Record<PluginPermission, (typeof messages)[keyof typeof messages]>

function failure(cause: unknown) {
    error.value = cause instanceof Error ? cause.message : String(cause)
}

async function chooseManifest(event: Event) {
    const input = event.currentTarget as HTMLInputElement
    const file = input.files?.[0]
    input.value = ''
    if (!file) return
    error.value = ''
    notice.value = ''
    try {
        if (file.size > MAX_MANIFEST_BYTES)
            throw new Error('The plugin file exceeds the size limit.')
        pendingManifest.value = parseLocalPlugin(await file.text())
        pendingEnable.value = false
    } catch (cause) {
        failure(cause)
    }
}

function installReviewed() {
    const manifest = pendingManifest.value
    if (!manifest) return
    error.value = ''
    try {
        if (pendingEnable.value) plugins.setEnabled(manifest.id, true)
        else plugins.install(manifest)
        notice.value = formatMessage(messages.installed, { name: manifest.name })
        pendingManifest.value = null
        pendingEnable.value = false
    } catch (cause) {
        failure(cause)
    }
}

function toggle(plugin: InstalledLocalPlugin) {
    error.value = ''
    if (!plugin.enabled) {
        pendingManifest.value = plugin.manifest
        pendingEnable.value = true
        return
    }
    try {
        plugins.setEnabled(plugin.manifest.id, false)
        if (activeId.value === plugin.manifest.id) activeId.value = ''
    } catch (cause) {
        failure(cause)
    }
}

async function uninstall(plugin: InstalledLocalPlugin) {
    if (
        !(await confirm(formatMessage(messages.removeConfirm, { name: plugin.manifest.name }), {
            title: formatMessage(messages.removeTitle),
            kind: 'warning',
            okLabel: formatMessage(messages.remove),
            cancelLabel: formatMessage(messages.cancel),
        }))
    )
        return
    try {
        plugins.remove(plugin.manifest.id)
        if (activeId.value === plugin.manifest.id) activeId.value = ''
    } catch (cause) {
        failure(cause)
    }
}

async function action(plugin: InstalledLocalPlugin, actionId: string) {
    if (busy.value) return
    busy.value = true
    error.value = ''
    try {
        await runLocalPluginAction(plugin, actionId, host)
    } catch (cause) {
        failure(cause)
    } finally {
        busy.value = false
    }
}

async function applyTheme(plugin: InstalledLocalPlugin) {
    if (
        !plugin.manifest.theme ||
        !plugin.manifest.permissions.includes('ui.theme') ||
        !plugin.enabled
    )
        return
    busy.value = true
    error.value = ''
    try {
        await host.theme(plugin.manifest.theme)
        notice.value = formatMessage(messages.themeApplied)
    } catch (cause) {
        failure(cause)
    } finally {
        busy.value = false
    }
}

function downloadExample() {
    const url = URL.createObjectURL(
        new Blob([JSON.stringify(exampleLocalPlugin, null, 2)], { type: 'application/json' }),
    )
    const link = document.createElement('a')
    link.href = url
    link.download = 'manifest.json'
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}
</script>

<template>
    <main
        class="box-border flex min-h-full w-full flex-col gap-4 p-6"
        data-onboarding-id="plugins-manager"
    >
        <header class="flex flex-wrap items-start justify-between gap-3">
            <div>
                <h1 class="m-0 text-2xl font-semibold text-[var(--color-text-primary)]">
                    <PackageIcon class="mr-2 inline size-5 align-[-3px] text-brand" />
                    {{ formatMessage(messages.title) }}
                </h1>
                <p class="mb-0 mt-1 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.description) }}
                </p>
            </div>
            <div class="flex flex-wrap gap-2">
                <Button type="outlined" @click="guideVisible = !guideVisible">{{
                    formatMessage(messages.docs)
                }}</Button>
                <Button type="colored" color="brand" @click="picker?.click()"
                    ><UploadIcon />{{ formatMessage(messages.install) }}</Button
                >
                <input
                    ref="picker"
                    type="file"
                    accept=".json,application/json"
                    class="hidden"
                    :aria-label="formatMessage(messages.file)"
                    @change="chooseManifest"
                />
            </div>
        </header>

        <p v-if="error" class="m-0 whitespace-pre-wrap break-words text-sm text-red" role="alert">
            {{ error }}
        </p>
        <p v-if="notice" class="m-0 text-sm text-[var(--color-text-tertiary)]" role="status">
            {{ notice }}
        </p>

        <section
            v-if="guideVisible"
            class="flex max-w-4xl flex-col items-start gap-3 rounded-md border border-solid border-surface-5 bg-surface-2 p-4"
        >
            <h2 class="m-0 text-base font-semibold text-[var(--color-text-primary)]">
                {{ formatMessage(messages.guideTitle) }}
            </h2>
            <p class="m-0 text-sm leading-relaxed text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.guide) }}
            </p>
            <Button type="outlined" @click="downloadExample">{{
                formatMessage(messages.sample)
            }}</Button>
        </section>

        <section v-if="active?.manifest.page" class="flex min-w-0 flex-col gap-3" data-plugin-page>
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h2 class="m-0 text-lg font-semibold text-[var(--color-text-primary)]">
                    {{ active.manifest.page.title }}
                </h2>
                <Button type="outlined" @click="activeId = ''">{{
                    formatMessage(messages.back)
                }}</Button>
            </div>
            <div v-if="active.manifest.actions.length" class="flex flex-wrap gap-2">
                <Button
                    v-for="item in active.manifest.actions"
                    :key="item.id"
                    type="outlined"
                    :disabled="busy"
                    @click="action(active, item.id)"
                    >{{ item.label }}</Button
                >
            </div>
            <PluginSandbox
                :plugin="active"
                :content="active.manifest.page"
                :host="host"
                :height="560"
            />
        </section>

        <p
            v-if="!plugins.installed.value.length"
            class="m-0 rounded-md border border-dashed border-surface-5 p-6 text-sm text-[var(--color-text-tertiary)]"
        >
            {{ formatMessage(messages.empty) }}
        </p>
        <div v-else class="grid grid-cols-1 items-start gap-4 xl:grid-cols-2">
            <article
                v-for="plugin in plugins.installed.value"
                :key="plugin.manifest.id"
                :data-plugin-id="plugin.manifest.id"
                class="flex min-w-0 flex-col gap-3 rounded-lg border border-solid border-surface-5 bg-surface-2 p-4"
            >
                <div class="flex items-start justify-between gap-3">
                    <div class="min-w-0">
                        <h2
                            class="m-0 break-words text-lg font-semibold text-[var(--color-text-primary)]"
                        >
                            {{ plugin.manifest.name }}
                        </h2>
                        <p class="mb-0 mt-1 break-all text-xs text-[var(--color-text-tertiary)]">
                            {{ plugin.manifest.id }} · {{ plugin.manifest.version }}
                        </p>
                    </div>
                    <span
                        class="text-xs font-semibold"
                        :class="plugin.enabled ? 'text-brand' : 'text-[var(--color-text-tertiary)]'"
                        >{{
                            formatMessage(plugin.enabled ? messages.enabled : messages.disabled)
                        }}</span
                    >
                </div>
                <p
                    v-if="plugin.manifest.description"
                    class="m-0 break-words text-sm text-[var(--color-text-tertiary)]"
                >
                    {{ plugin.manifest.description }}
                </p>
                <ul
                    class="m-0 flex list-none flex-wrap gap-2 p-0 text-xs text-[var(--color-text-tertiary)]"
                >
                    <li v-if="plugin.manifest.page">{{ formatMessage(messages.page) }}</li>
                    <li v-if="plugin.manifest.theme">{{ formatMessage(messages.themeFeature) }}</li>
                    <li v-if="plugin.manifest.panels.length">
                        {{
                            formatMessage(messages.panels, { count: plugin.manifest.panels.length })
                        }}
                    </li>
                    <li v-if="plugin.manifest.actions.length">
                        {{
                            formatMessage(messages.actions, {
                                count: plugin.manifest.actions.length,
                            })
                        }}
                    </li>
                </ul>
                <details class="text-xs text-[var(--color-text-tertiary)]">
                    <summary class="cursor-pointer font-semibold">
                        {{ formatMessage(messages.permissions) }}
                    </summary>
                    <ul v-if="plugin.manifest.permissions.length" class="mb-0 mt-2 pl-5">
                        <li v-for="permission in plugin.manifest.permissions" :key="permission">
                            {{ formatMessage(permissionMessages[permission]) }}
                        </li>
                    </ul>
                    <p v-else class="mb-0 mt-2">{{ formatMessage(messages.noPermissions) }}</p>
                </details>
                <div class="flex flex-wrap gap-2">
                    <Button
                        type="outlined"
                        :disabled="busy"
                        :data-plugin-toggle="plugin.manifest.id"
                        @click="toggle(plugin)"
                        >{{
                            formatMessage(plugin.enabled ? messages.disable : messages.enable)
                        }}</Button
                    >
                    <Button
                        v-if="plugin.enabled && plugin.manifest.page"
                        type="outlined"
                        :data-plugin-open="plugin.manifest.id"
                        @click="activeId = plugin.manifest.id"
                        >{{ formatMessage(messages.open) }}</Button
                    >
                    <Button
                        v-if="plugin.enabled && plugin.manifest.theme"
                        type="outlined"
                        :disabled="busy"
                        @click="applyTheme(plugin)"
                        >{{ formatMessage(messages.theme) }}</Button
                    >
                    <Button type="outlined" :disabled="busy" @click="uninstall(plugin)">{{
                        formatMessage(messages.remove)
                    }}</Button>
                </div>
                <div
                    v-if="plugin.enabled && !plugin.manifest.page && plugin.manifest.actions.length"
                    class="flex flex-wrap gap-2 border-t border-solid border-surface-4 pt-3"
                >
                    <Button
                        v-for="item in plugin.manifest.actions"
                        :key="item.id"
                        type="outlined"
                        :disabled="busy"
                        @click="action(plugin, item.id)"
                        >{{ item.label }}</Button
                    >
                </div>
            </article>
        </div>

        <div
            v-if="pendingManifest"
            class="fixed inset-0 z-[500] flex items-center justify-center bg-black/60 p-4"
            role="presentation"
            @click.self="pendingManifest = null"
        >
            <section
                class="flex max-h-[85vh] w-full max-w-xl flex-col gap-3 overflow-y-auto rounded-lg border border-solid border-surface-5 bg-surface-2 p-4 shadow-xl"
                role="dialog"
                aria-modal="true"
                aria-labelledby="plugin-review-title"
                data-plugin-review
            >
                <h2
                    id="plugin-review-title"
                    class="m-0 text-lg font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(pendingEnable ? messages.reviewEnable : messages.review) }}
                </h2>
                <p class="m-0 break-words font-semibold text-[var(--color-text-primary)]">
                    {{ pendingManifest.name }} · {{ pendingManifest.version }}
                </p>
                <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.reviewHint) }}
                </p>
                <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                    {{ formatMessage(messages.permissions) }}
                </h3>
                <ul
                    v-if="pendingManifest.permissions.length"
                    class="m-0 pl-5 text-sm text-[var(--color-text-tertiary)]"
                >
                    <li v-for="permission in pendingManifest.permissions" :key="permission">
                        {{ formatMessage(permissionMessages[permission]) }}
                    </li>
                </ul>
                <p v-else class="m-0 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.noPermissions) }}
                </p>
                <div class="flex flex-wrap justify-end gap-2">
                    <Button type="outlined" @click="pendingManifest = null">{{
                        formatMessage(messages.cancel)
                    }}</Button>
                    <Button
                        type="colored"
                        color="brand"
                        data-plugin-install-confirm
                        @click="installReviewed"
                        >{{
                            formatMessage(
                                pendingEnable
                                    ? messages.enable
                                    : replacing
                                      ? messages.confirmReplace
                                      : messages.confirmInstall,
                            )
                        }}</Button
                    >
                </div>
            </section>
        </div>
    </main>
</template>
