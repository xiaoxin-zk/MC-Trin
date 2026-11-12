<script setup lang="ts">
import { EyeIcon } from '@modrinth/assets'
import { Button, defineMessages, useVIntl } from '@modrinth/ui'
import { getVersion } from '@tauri-apps/api/app'
import { ref } from 'vue'

import UpdateAnnouncementHistory from '@/components/ui/announcement/UpdateAnnouncementHistory.vue'
import UpdateAnnouncementModal from '@/components/ui/announcement/UpdateAnnouncementModal.vue'
import { isDev } from '@/helpers/utils'

import SettingsRow from './SettingsRow.vue'
import SettingsSection from './SettingsSection.vue'

const [currentVersion, devBuild] = await Promise.all([getVersion(), isDev()])
const previewModal = ref<InstanceType<typeof UpdateAnnouncementModal>>()

const { formatMessage } = useVIntl()

const messages = defineMessages({
    previewTitle: {
        id: 'app.settings.updates.preview.title',
        defaultMessage: 'Announcement preview',
    },
    previewDescription: {
        id: 'app.settings.updates.preview.description',
        defaultMessage:
            'Shows the dialog users see after updating. Nothing is marked as shown, and the pending announcement is left untouched.',
    },
    previewOpen: {
        id: 'app.settings.updates.preview.open',
        defaultMessage: 'Preview announcement',
    },
})

function openPreview() {
    previewModal.value?.show(currentVersion)
}
</script>

<template>
    <div class="flex flex-col gap-6">
        <UpdateAnnouncementHistory :current-version="currentVersion" />

        <template v-if="devBuild">
            <SettingsSection :title="formatMessage(messages.previewTitle)">
                <SettingsRow>
                    <template #description>
                        {{ formatMessage(messages.previewDescription) }}
                    </template>
                    <template #control>
                        <Button type="outlined" color="brand" @click="openPreview">
                            <EyeIcon />
                            {{ formatMessage(messages.previewOpen) }}
                        </Button>
                    </template>
                </SettingsRow>
            </SettingsSection>
            <UpdateAnnouncementModal ref="previewModal" />
        </template>
    </div>
</template>
