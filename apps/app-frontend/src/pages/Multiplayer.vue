<script setup lang="ts">
import { GlobeIcon, UsersIcon } from '@modrinth/assets'
import { defineMessages, NavTabs, useVIntl } from '@modrinth/ui'
import { computed } from 'vue'
import { RouterView, useRoute, useRouter } from 'vue-router'

const { formatMessage } = useVIntl()
const route = useRoute()
const router = useRouter()

const messages = defineMessages({
    title: { id: 'app.multiplayer.title', defaultMessage: 'Multiplayer' },
    roomsTab: { id: 'app.multiplayer.tab.rooms', defaultMessage: 'Rooms' },
    publicLobbiesTab: {
        id: 'app.multiplayer.tab.public-lobbies',
        defaultMessage: 'Public lobbies',
    },
})

const activeTab = computed(() =>
    route.path.startsWith('/multiplayer/public-lobbies') ? 'public-lobbies' : 'rooms',
)
// 服务器详情页用固定高度布局：控制台内部滚动，命令输入框始终可见
const isStudioMode = computed(() => route.name === 'MultiplayerServerFileStudio')
const isFixedRender = computed(
    () => route.name === 'MultiplayerServerDetail' || route.name === 'MultiplayerServerFileStudio',
)
const tabLinks = computed(() => [
    { label: formatMessage(messages.roomsTab), href: '/multiplayer/rooms', icon: UsersIcon },
    {
        label: formatMessage(messages.publicLobbiesTab),
        href: '/multiplayer/public-lobbies',
        icon: GlobeIcon,
    },
])

function handleTabClick(index: number) {
    void router.push(tabLinks.value[index]?.href ?? '/multiplayer/rooms')
}
</script>

<template>
    <div
        :class="
            isStudioMode
                ? 'flex h-full min-h-0 w-full flex-col'
                : isFixedRender
                  ? 'box-border flex h-full min-h-0 w-full flex-col gap-3 p-6'
                  : 'box-border flex min-h-full w-full flex-col gap-3 p-6'
        "
    >
        <template v-if="!isStudioMode">
            <h1 class="m-0 shrink-0 text-2xl font-semibold text-[var(--color-text-primary)]">
                {{ formatMessage(messages.title) }}
            </h1>
            <!--
				This bar drives routes rather than panels, so it has no tabpanel of
				its own: the outlet below renders child routes, and the detail and
				rooms views own their own tablists. Navigation mode would fit the
				routes but renders links rather than buttons, which normalise
				underlines, so the switch waits on a visual review.
			-->
            <NavTabs
                mode="local"
                :active-index="activeTab === 'public-lobbies' ? 1 : 0"
                :links="tabLinks"
                @tab-click="handleTabClick"
            />
        </template>

        <RouterView />
    </div>
</template>
