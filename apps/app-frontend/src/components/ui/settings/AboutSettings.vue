<script setup lang="ts">
import { defineMessages, useVIntl } from '@modrinth/ui'

import SettingsSection from './SettingsSection.vue'

const { formatMessage } = useVIntl()

const messages = defineMessages({
    author: {
        id: 'app.settings.about.author',
        defaultMessage: 'Author',
    },
})
</script>

<template>
    <SettingsSection :title="formatMessage(messages.author)">
        <p class="m-0 p-4 font-semibold text-[var(--color-text-primary)]">xiaoxinZK</p>
    </SettingsSection>
</template>
