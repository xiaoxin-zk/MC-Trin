<template>
    <RouterLink
        v-if="typeof to === 'string' && !disabled"
        :to="to"
        v-bind="$attrs"
        active-class="nav-router-active"
        :class="[activeClasses, buttonLayoutClasses]"
    >
        <slot />
        <span
            v-if="expanded && label"
            class="min-w-0 flex-1 truncate text-left text-sm font-medium"
        >
            {{ label }}
        </span>
    </RouterLink>
    <button
        v-else-if="typeof to === 'string'"
        v-bind="$attrs"
        type="button"
        aria-disabled="true"
        tabindex="-1"
        :class="[activeClasses, buttonLayoutClasses]"
        @click.prevent
        @keydown.enter.prevent
        @keyup.enter.prevent
        @keydown.space.prevent
        @keyup.space.prevent
    >
        <slot />
        <span
            v-if="expanded && label"
            class="min-w-0 flex-1 truncate text-left text-sm font-medium"
        >
            {{ label }}
        </span>
    </button>
    <button
        v-else
        v-bind="$attrs"
        :class="[buttonLayoutClasses, 'button-animation border-none cursor-pointer']"
        :disabled="disabled"
        @click="to"
    >
        <slot />
        <span
            v-if="expanded && label"
            class="min-w-0 flex-1 truncate text-left text-sm font-medium"
        >
            {{ label }}
        </span>
    </button>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { RouteLocationNormalizedLoaded } from 'vue-router'
import { RouterLink, useLink, useRoute } from 'vue-router'

const route = useRoute()

type RouteFunction = (route: RouteLocationNormalizedLoaded) => boolean

const props = withDefaults(
    defineProps<{
        to: (() => void) | string
        isPrimary?: RouteFunction
        isSubpage?: RouteFunction
        highlightOverride?: boolean
        disabled?: boolean
        expanded?: boolean
        label?: string
    }>(),
    {
        disabled: false,
        expanded: false,
        isPrimary: undefined,
        label: '',
        isSubpage: undefined,
    },
)

const link = useLink({ to: computed(() => (typeof props.to === 'string' ? props.to : '/')) })
const buttonLayoutClasses = computed(() =>
    props.expanded
        ? 'flex h-10 w-full min-w-0 items-center justify-start gap-3 rounded-md px-3 text-base text-[var(--color-text-default)] transition-colors bg-transparent hover:bg-surface-4 hover:text-[var(--color-text-primary)] [&>svg]:size-6 [&>svg]:shrink-0'
        : 'flex h-12 w-12 shrink-0 items-center justify-center rounded-full text-2xl text-[var(--color-text-default)] transition-all bg-transparent hover:bg-surface-4 hover:text-[var(--color-text-primary)] [&>svg]:shrink-0',
)
const activeClasses = computed(() => {
    const primary = props.isPrimary ? props.isPrimary(route) : link.isActive.value
    return {
        'router-link-active': primary,
        'subpage-active': !primary && !!props.isSubpage?.(route),
    }
})

defineOptions({
    inheritAttrs: false,
})
</script>

<style lang="scss" scoped>
.router-link-active,
.subpage-active {
    svg {
        filter: drop-shadow(0 0 0.5rem black);
    }
}

.router-link-active {
    @apply text-[--color-button-text-selected] bg-[--color-button-bg-selected];
}

.subpage-active {
    @apply text-[var(--color-text-primary)] bg-surface-4;
}
</style>
