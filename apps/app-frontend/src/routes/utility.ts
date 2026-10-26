import './meta'

import type { RouteRecordRaw } from 'vue-router'

/** Top-level utility pages that do not belong to a larger content vertical. */
export const utilityRoutes: RouteRecordRaw[] = [
    {
        path: '/plugins',
        name: 'Plugins',
        component: () => import('@/pages/Plugins.vue'),
        meta: { breadcrumb: [{ name: 'Plugins' }], discordActivity: 'Idling...' },
    },
    {
        path: '/agent',
        name: 'AIAgent',
        component: () => import('@/pages/AIAgent.vue'),
        meta: {
            breadcrumb: [{ name: 'AI assistant' }],
            discordActivity: 'Idling...',
        },
    },
    {
        path: '/worlds',
        name: 'Worlds',
        component: () => import('@/pages/Worlds.vue'),
        meta: {
            breadcrumb: [{ name: 'Worlds' }],
        },
    },
    {
        path: '/create',
        name: 'Create',
        component: () => import('@/pages/Create.vue'),
        meta: { useRootContext: false },
    },
    {
        path: '/downloads',
        name: 'Downloads',
        component: () => import('@/pages/Downloads.vue'),
        meta: {
            breadcrumb: [{ name: 'Downloads' }],
            discordActivity: 'Idling...',
        },
    },
    {
        path: '/settings',
        name: 'Settings',
        component: () => import('@/pages/Settings.vue'),
        meta: {
            breadcrumb: [{ name: 'Settings' }],
            discordActivity: 'Idling...',
        },
    },
    {
        path: '/skins',
        name: 'SkinSelector',
        component: () => import('@/pages/Skins.vue'),
        meta: {
            breadcrumb: [{ name: 'Skin selector' }],
            discordActivity: 'Changing skins...',
        },
    },
    {
        path: '/screenshots',
        name: 'Screenshots',
        component: () => import('@/pages/Screenshots.vue'),
        meta: { breadcrumb: [{ name: 'Screenshots' }] },
    },
    {
        path: '/help/drop',
        name: 'DropHelp',
        component: () => import('@/pages/help/DropHelp.vue'),
        meta: {
            breadcrumb: [{ name: 'Drop help' }],
        },
    },
]
