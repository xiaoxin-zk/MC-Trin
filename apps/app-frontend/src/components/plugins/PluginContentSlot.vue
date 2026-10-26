<script setup lang="ts">
import { computed } from 'vue'

import { useLocalPlugins } from '@/plugins/local-plugin-store'
import { usePluginHost } from '@/plugins/use-plugin-host'

import PluginSandbox from './PluginSandbox.vue'

const props = defineProps<{ name: 'home.after' }>()
const { installed } = useLocalPlugins()
const host = usePluginHost()
const panels = computed(() =>
    installed.value
        .filter((plugin) => plugin.enabled)
        .flatMap((plugin) =>
            plugin.manifest.panels
                .filter((panel) => panel.slot === props.name)
                .map((panel) => ({ plugin, panel })),
        ),
)
</script>

<template>
    <div
        v-if="panels.length"
        class="grid w-full grid-cols-1 gap-4 lg:grid-cols-2"
        data-plugin-content-slot="home.after"
    >
        <section
            v-for="{ plugin, panel } in panels"
            :key="`${plugin.manifest.id}:${panel.id}`"
            class="flex min-w-0 flex-col gap-2"
        >
            <h2 class="m-0 text-base font-semibold text-[var(--color-text-primary)]">
                {{ panel.title }}
            </h2>
            <PluginSandbox :plugin="plugin" :content="panel" :host="host" :height="panel.height" />
        </section>
    </div>
</template>
