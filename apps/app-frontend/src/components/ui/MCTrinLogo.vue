<template>
    <div
        aria-label="MC-Trin"
        class="flex h-full items-center gap-2 font-extrabold text-[var(--color-text-primary)]"
    >
        <img aria-hidden="true" class="aspect-square h-full object-contain" :src="mctrinVisual" />
        <span v-if="!iconOnly" class="hidden text-sm tracking-wide xl:inline">MC-Trin</span>
    </div>
</template>

<script setup lang="ts">
const mctrinVisual = '/mctrin.svg'

defineProps<{
    iconOnly?: boolean
}>()
</script>
