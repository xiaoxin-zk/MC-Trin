<script setup lang="ts">
import { defineMessages, useVIntl } from '@modrinth/ui'

import { audioThirdPartyComponents } from '@/data/about/audio-third-party'

import SettingsSection from './SettingsSection.vue'

const { formatMessage } = useVIntl()

const messages = defineMessages({
    author: {
        id: 'app.settings.about.author',
        defaultMessage: 'Author',
    },
    audioComponents: {
        id: 'app.settings.about.audio-components',
        defaultMessage: 'Audio components',
    },
})
</script>

<template>
    <SettingsSection :title="formatMessage(messages.author)">
        <p class="m-0 p-4 font-semibold text-[var(--color-text-primary)]">xiaoxinZK</p>
    </SettingsSection>
    <SettingsSection :title="formatMessage(messages.audioComponents)">
        <ul class="m-0 flex list-none flex-col gap-2 p-4">
            <li
                v-for="component in audioThirdPartyComponents"
                :key="component.name"
                class="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-sm"
            >
                <span class="font-semibold text-[var(--color-text-primary)]">
                    {{ component.name }}
                </span>
                <span class="text-[var(--color-text-tertiary)]">{{ component.licence }}</span>
                <span class="min-w-0 text-[var(--color-text-tertiary)]">
                    {{ component.purpose }}
                </span>
            </li>
        </ul>
    </SettingsSection>
</template>
