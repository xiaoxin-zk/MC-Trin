import './meta'

import type { RouteRecordRaw } from 'vue-router'

export const multiplayerRoutes: RouteRecordRaw[] = [
    {
        path: '/multiplayer',
        // Layout route is unnamed so vue-router does not warn about the empty
        // redirect child; navigation uses MultiplayerServers / child names.
        component: () => import('@/pages/Multiplayer.vue'),
        meta: {
            breadcrumb: [{ name: 'Multiplayer' }],
            discordActivity: 'Idling...',
            pageTransitionGroup: 'multiplayer',
        },
        children: [
            {
                path: '',
                redirect: { name: 'MultiplayerRooms' },
            },
            {
                path: 'servers/:pathMatch(.*)*',
                redirect: { name: 'MultiplayerRooms' },
            },
            {
                path: 'servers',
                redirect: { name: 'MultiplayerRooms' },
            },
            {
                path: 'rooms',
                name: 'MultiplayerRooms',
                component: () => import('@/components/multiplayer/MultiplayerRooms.vue'),
            },
            {
                path: 'public-lobbies',
                name: 'MultiplayerPublicLobbies',
                component: () => import('@/components/multiplayer/PublicLobbies.vue'),
            },
        ],
    },
]
