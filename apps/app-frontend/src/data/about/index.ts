export interface TeamMember {
	name: string
	avatar: string
	avatarRemote?: string
	url?: string
	experience?: string
}

export interface Contributor {
	name: string
	avatarUrl: string
	url: string
	contributions: number
}

// MC-Trin intentionally exposes only its author on the About page. Keep the
// typed exports for plugin compatibility without loading the upstream team or
// contributor catalog into the application bundle.
export const teamMembers: (TeamMember & { avatarUrl: string })[] = []
export const contributors: Contributor[] = []
