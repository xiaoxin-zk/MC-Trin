import type { Component } from 'vue'

export type AboutMemberExperience = {
    component: Component
    longPressDuration: number
}

// The about page no longer exposes the upstream mascot mini-games. Keeping the
// resolver allows older contributor data to remain readable without rendering
// legacy project material.
const memberExperiences: Record<string, AboutMemberExperience> = {}

export function getAboutMemberExperience(experience: unknown): AboutMemberExperience | undefined {
    return typeof experience === 'string' ? memberExperiences[experience] : undefined
}
