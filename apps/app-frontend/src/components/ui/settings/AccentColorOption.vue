<script setup lang="ts">
import { CheckIcon } from '@modrinth/assets'

defineProps<{
    label: string
    accessibleLabel?: string
    description?: string
    background: string
    selected: boolean
    disabled?: boolean
}>()
</script>

<template>
    <button
        v-tooltip="label"
        type="button"
        role="radio"
        :aria-label="accessibleLabel ?? label"
        :aria-checked="selected"
        :disabled="disabled"
        class="relative flex min-w-0 flex-1 items-center gap-2 overflow-hidden rounded-lg border border-solid py-2.5 pl-2 pr-6 font-semibold transition-[background-color,border-color,color,transform] enabled:active:scale-[0.97]"
        :class="
            disabled
                ? 'cursor-not-allowed border-surface-4 bg-surface-2 text-[var(--color-text-tertiary)] opacity-60'
                : selected
                  ? 'border-brand bg-brand-highlight text-brand'
                  : 'border-surface-4 bg-surface-3 text-[var(--color-text-tertiary)] hover:border-surface-5 hover:text-[var(--color-text-primary)]'
        "
    >
        <span
            class="size-4 shrink-0 rounded-full ring-2 ring-white/20"
            :style="{ background }"
            aria-hidden="true"
        />
        <!--
            The check overlays the top-right corner instead of taking a grid
            column, and the button's `pr-6` keeps the label clear of it, so the
            chip spends its width on text instead of a 14px track plus a gap.
        -->
        <span class="flex min-w-0 flex-1 flex-col text-start leading-tight">
            <span class="truncate">{{ label }}</span>
            <span v-if="description" class="truncate text-xs font-normal">{{ description }}</span>
        </span>
        <CheckIcon
            class="absolute right-0.5 top-0.5 size-3.5"
            :class="selected ? 'opacity-100' : 'opacity-0'"
            aria-hidden="true"
        />
    </button>
</template>
