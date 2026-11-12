<template>
    <div>
        <ProjectPageVersions
            :loaders="loaders"
            :game-versions="gameVersions"
            :versions="versions"
            :project="project"
            :show-environment-column="themeStore.featureFlags.show_version_environment_column"
            :version-link="
                (version) => buildProjectHref(`/project/${project.id}/version/${version.id}`)
            "
        >
            <template #actions="{ version }">
                <Button
                    v-tooltip="
                        !installed
                            ? formatMessage(commonMessages.installButton)
                            : version.id !== installedVersion
                              ? formatMessage(commonMessages.switchToVersionButton)
                              : formatMessage(messages.alreadyInstalled)
                    "
                    circular
                    icon-only
                    type="quiet"
                    :color="installed && version.id === installedVersion ? 'base' : 'green'"
                    :disabled="installing || (installed && version.id === installedVersion)"
                    @click.stop="() => install(version.id)"
                >
                    <DownloadIcon v-if="!installed" />
                    <SwapIcon v-else-if="installed && version.id !== installedVersion" />
                    <CheckIcon v-else />
                </Button>
                <ButtonLink
                    v-tooltip="formatMessage(commonMessages.openInBrowserButton)"
                    :href="`https://modrinth.com/${project.project_type}/${project.slug}/version/${version.id}`"
                    target="_blank"
                    type="quiet"
                    circular
                    icon-only
                    :label="formatMessage(commonMessages.openInBrowserButton)"
                >
                    <ExternalIcon />
                </ButtonLink>
            </template>
        </ProjectPageVersions>
    </div>
</template>

<script setup>
import { CheckIcon, DownloadIcon, ExternalIcon } from '@modrinth/assets'
import {
    Button,
    ButtonLink,
    commonMessages,
    defineMessages,
    injectNotificationManager,
    ProjectPageVersions,
    useVIntl,
} from '@modrinth/ui'
import { ref } from 'vue'
import { useRoute } from 'vue-router'

import { SwapIcon } from '@/assets/icons/index.js'
import { get_game_versions, get_loaders } from '@/helpers/tags.js'
import { useTheming } from '@/store/theme.ts'

const { formatMessage } = useVIntl()
const themeStore = useTheming()

const messages = defineMessages({
    alreadyInstalled: {
        id: 'app.project.versions.already-installed',
        defaultMessage: 'Already installed',
    },
})

defineProps({
    project: {
        type: Object,
        default: () => {},
    },
    versions: {
        type: Array,
        required: true,
    },
    install: {
        type: Function,
        required: true,
    },
    installed: {
        type: Boolean,
        default: null,
    },
    installing: {
        type: Boolean,
        default: false,
    },
    installedVersion: {
        type: String,
        default: null,
    },
})

const { handleError } = injectNotificationManager()
const route = useRoute()

function buildProjectHref(path) {
    const params = new URLSearchParams()
    for (const [key, val] of Object.entries(route.query)) {
        if (Array.isArray(val)) {
            for (const v of val) params.append(key, v)
        } else if (val) {
            params.append(key, String(val))
        }
    }
    const qs = params.toString()
    return qs ? `${path}?${qs}` : path
}

const [loaders, gameVersions] = await Promise.all([
    get_loaders().catch(handleError).then(ref),
    get_game_versions().catch(handleError).then(ref),
])
</script>

<style scoped lang="scss">
.table-row {
    grid-template-columns: min-content 1fr 1fr 1.5fr;
}

.select {
    width: 100% !important;
    max-width: 20rem;
}

.version-link {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    text-wrap: wrap;

    .version-badge {
        display: flex;
        flex-wrap: wrap;
    }
}

.filter-checkbox {
    :deep(.checkbox) {
        border: none;
    }
}
</style>
