<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

import {
    buildLocalPluginDocument,
    dispatchLocalPluginRequest,
    type InstalledLocalPlugin,
    isPluginRequest,
    type PluginHost,
    type PluginPage,
} from '@/plugins/local-plugin-runtime'

const props = withDefaults(
    defineProps<{
        plugin: InstalledLocalPlugin
        content: PluginPage
        host: PluginHost
        height?: number
    }>(),
    { height: 420 },
)

const frame = ref<HTMLIFrameElement>()
const session = ref('')
const documentSource = ref('')
const closed = ref(false)
const scriptError = ref('')
const seen = new Set<string>()
let active = true
let pending = 0
let ready = false
let loaded = false

watch(
    () => [props.plugin.manifest, props.content, props.plugin.enabled],
    () => {
        closed.value = false
        scriptError.value = ''
        ready = false
        loaded = false
        session.value = crypto.randomUUID().replaceAll('-', '')
        documentSource.value = props.plugin.enabled
            ? buildLocalPluginDocument(
                  props.content,
                  session.value,
                  new URL('/mctrin-plugin-bootstrap.js', window.location.href).href,
              )
            : ''
        seen.clear()
        pending = 0
    },
    { immediate: true },
)

const heightStyle = computed(() => `${Math.min(900, Math.max(120, props.height))}px`)

async function receive(event: MessageEvent) {
    const source = frame.value?.contentWindow
    if (
        !active ||
        !source ||
        event.source !== source ||
        event.origin !== 'null' ||
        !props.plugin.enabled ||
        closed.value ||
        event.data?.session !== session.value
    )
        return
    if (event.data.type === 'mctrin:closing') {
        ready = false
        closed.value = true
        return
    }
    if (event.data.type === 'mctrin:ready') {
        ready = true
        return
    }
    if (event.data.type === 'mctrin:error') {
        scriptError.value = String(event.data.message ?? '').slice(0, 300)
        return
    }
    if (!ready || !isPluginRequest(event.data, session.value)) return
    const request = event.data
    if (seen.has(request.requestId)) return
    if (pending >= 20 || seen.size >= 1000) {
        source.postMessage(
            {
                type: 'mctrin:response',
                requestId: request.requestId,
                session: request.session,
                ok: false,
                value: 'Plugin request limit reached.',
            },
            '*',
        )
        return
    }
    seen.add(request.requestId)
    pending++
    const response = await dispatchLocalPluginRequest(props.plugin, request, props.host)
    pending = Math.max(0, pending - 1)
    if (
        active &&
        ready &&
        !closed.value &&
        props.plugin.enabled &&
        source === frame.value?.contentWindow &&
        request.session === session.value
    )
        source.postMessage(response, '*')
}

function frameLoaded() {
    if (loaded) {
        ready = false
        closed.value = true
    }
    loaded = true
}

onMounted(() => window.addEventListener('message', receive))
onUnmounted(() => {
    active = false
    window.removeEventListener('message', receive)
})
</script>

<template>
    <iframe
        v-if="plugin.enabled && !closed"
        ref="frame"
        :key="session"
        :title="content.title"
        :srcdoc="documentSource"
        sandbox="allow-scripts"
        allow=""
        referrerpolicy="no-referrer"
        credentialless
        class="box-border w-full rounded-md border border-solid border-surface-5 bg-white"
        :style="{ height: heightStyle }"
        @load="frameLoaded"
    />
    <p v-if="scriptError" class="m-0 whitespace-pre-wrap break-words text-sm text-red" role="alert">
        {{ scriptError }}
    </p>
</template>
