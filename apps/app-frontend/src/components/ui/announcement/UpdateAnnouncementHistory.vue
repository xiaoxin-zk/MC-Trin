<script setup lang="ts">
import { CalendarIcon, HistoryIcon } from '@modrinth/assets'
import { Accordion, defineMessages, TagItem, useVIntl } from '@modrinth/ui'
import { computed } from 'vue'

import {
    getAnnouncementByVersion,
    getAnnouncements,
    getLocalizedAnnouncementText,
} from '@/announcements/catalog'

import UpdateAnnouncementContent from './UpdateAnnouncementContent.vue'

const props = defineProps<{
    currentVersion: string
}>()

const { formatMessage, locale } = useVIntl()

const messages = defineMessages({
    title: {
        id: 'app.settings.updates.announcements.title',
        defaultMessage: 'Update announcements',
    },
    description: {
        id: 'app.settings.updates.announcements.description',
        defaultMessage: 'See what changed in this version and browse previous releases.',
    },
    history: {
        id: 'app.settings.updates.announcements.history',
        defaultMessage: 'Version history',
    },
    empty: {
        id: 'app.settings.updates.announcements.empty',
        defaultMessage: 'No bundled update announcements are available.',
    },
})

const launcherAnnouncements = getAnnouncements()
const currentAnnouncement = computed(() => getAnnouncementByVersion(props.currentVersion))
const historyAnnouncements = computed(() =>
    launcherAnnouncements.filter(
        (announcement) => announcement.id !== currentAnnouncement.value?.id,
    ),
)
</script>

<template>
    <section
        class="flex min-w-0 flex-col gap-6 p-6 border border-surface-4 rounded-[var(--radius-md)] bg-surface-2"
    >
        <div class="flex min-w-0 flex-col gap-1">
            <h2 class="m-0 text-lg font-semibold text-[var(--color-text-primary)]">
                {{ formatMessage(messages.title) }}
            </h2>
            <p class="m-0 leading-relaxed text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.description) }}
            </p>
        </div>

        <div class="min-w-0">
            <UpdateAnnouncementContent
                :announcement="currentAnnouncement"
                :version="currentVersion"
            />
        </div>

        <div class="flex min-w-0 flex-col gap-3">
            <h3
                class="m-0 flex items-center gap-2 text-base font-semibold text-[var(--color-text-primary)]"
            >
                <HistoryIcon aria-hidden="true" class="size-4 text-[var(--color-text-tertiary)]" />
                {{ formatMessage(messages.history) }}
            </h3>
            <p
                v-if="historyAnnouncements.length === 0"
                class="m-0 text-sm text-[var(--color-text-tertiary)]"
            >
                {{ formatMessage(messages.empty) }}
            </p>
            <div v-else class="flex min-w-0 flex-col gap-2">
                <Accordion
                    v-for="announcement in historyAnnouncements"
                    :key="announcement.id"
                    class="min-w-0 overflow-hidden border border-surface-4 rounded-[var(--radius-sm)] bg-surface-3 transition-colors hover:border-surface-4 focus-within:border-surface-4"
                    button-class="group flex w-full cursor-pointer items-center gap-3 border-0 bg-transparent px-4 py-3 text-left"
                >
                    <template #title>
                        <div class="flex min-w-0 flex-1 items-center gap-3">
                            <div class="flex min-w-0 flex-1 flex-col gap-1">
                                <span
                                    class="truncate font-semibold text-[var(--color-text-default)] transition-colors group-hover:text-[var(--color-text-primary)]"
                                >
                                    {{ getLocalizedAnnouncementText(announcement.title, locale) }}
                                </span>
                                <div
                                    class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-[var(--color-text-tertiary)]"
                                >
                                    <TagItem class="px-1.5 py-0.5 text-xs"
                                        >v{{ announcement.version }}</TagItem
                                    >
                                    <span class="flex items-center gap-1">
                                        <CalendarIcon aria-hidden="true" class="size-3.5" />
                                        <time :datetime="announcement.publishedAt">{{
                                            announcement.publishedAt
                                        }}</time>
                                    </span>
                                </div>
                            </div>
                        </div>
                    </template>
                    <div class="update-announcement-history-item-content p-4">
                        <UpdateAnnouncementContent
                            :announcement="announcement"
                            :show-header="false"
                        />
                    </div>
                </Accordion>
            </div>
        </div>
    </section>
</template>

<style scoped>
.update-announcement-history-item-content {
    border-top: 1px solid
        var(--settings-divider, color-mix(in srgb, var(--surface-4) 55%, transparent));
}
</style>
