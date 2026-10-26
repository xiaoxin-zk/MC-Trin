<script setup lang="ts">
import { ExternalIcon } from '@modrinth/assets'
import { defineMessages, useVIntl } from '@modrinth/ui'
import { getVersion } from '@tauri-apps/api/app'
import { onMounted, ref } from 'vue'

import { MCTrinBrandConfig } from '@/config'

import SettingsSection from './SettingsSection.vue'

const { formatMessage } = useVIntl()
const version = ref('')

const messages = defineMessages({
    productTitle: {
        id: 'app.settings.about.product-title',
        defaultMessage: 'About {productName}',
    },
    productDescription: {
        id: 'app.settings.about.description',
        defaultMessage: 'A Minecraft Java Edition launcher.',
    },
    version: {
        id: 'app.settings.about.version',
        defaultMessage: 'Version {version}',
    },
    author: {
        id: 'app.settings.about.author',
        defaultMessage: 'Author',
    },
    licenseAttribution: {
        id: 'app.settings.about.license-attribution',
        defaultMessage: 'License and attribution',
    },
    attribution: {
        id: 'app.settings.about.attribution',
        defaultMessage:
            'MC-Trin is distributed under GPL-3.0. Copyright and third-party license notices remain in the project sources.',
    },
    projectLicense: {
        id: 'app.settings.about.project-license',
        defaultMessage: 'GPL-3.0 license',
    },
    thirdPartyNotice: {
        id: 'app.settings.about.third-party-notice',
        defaultMessage: 'The source notices retain copyright and third-party license terms.',
    },
})

onMounted(() => {
    void getVersion()
        .then((resolved) => {
            version.value = resolved
        })
        .catch(() => undefined)
})
</script>

<template>
    <div class="flex flex-col gap-6">
        <section id="settings-target-about-product" tabindex="-1" class="about-panel">
            <h2 class="m-0 text-xl font-semibold text-[var(--color-text-primary)]">
                {{
                    formatMessage(messages.productTitle, {
                        productName: MCTrinBrandConfig.productName,
                    })
                }}
            </h2>
            <p class="m-0 mt-1 text-[var(--color-text-tertiary)]">
                {{ formatMessage(messages.version, { version }) }}
            </p>
            <p class="m-0 mt-3 text-[var(--color-text-default)]">
                {{ formatMessage(messages.productDescription) }}
            </p>
        </section>

        <SettingsSection :title="formatMessage(messages.author)">
            <p class="m-0 p-4 font-semibold text-[var(--color-text-primary)]">xiaoxinZK</p>
        </SettingsSection>

        <SettingsSection :title="formatMessage(messages.licenseAttribution)">
            <div class="flex flex-col gap-3 p-4">
                <p class="m-0 text-[var(--color-text-default)]">
                    {{ formatMessage(messages.attribution) }}
                </p>
                <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.thirdPartyNotice) }}
                </p>
                <a
                    href="https://www.gnu.org/licenses/gpl-3.0.html"
                    target="_blank"
                    rel="noopener noreferrer"
                    class="inline-flex w-fit items-center gap-2 rounded-lg bg-surface-4 px-3 py-2 text-sm font-semibold text-[var(--color-text-primary)] transition-colors hover:bg-surface-5"
                >
                    {{ formatMessage(messages.projectLicense) }}
                    <ExternalIcon class="size-4 text-[var(--color-text-tertiary)]" />
                </a>
            </div>
        </SettingsSection>
    </div>
</template>

<style scoped>
.about-panel {
    padding: 1.25rem;
    border: 1px solid var(--surface-4);
    border-radius: var(--radius-md);
    background: var(--surface-3);
}
</style>
