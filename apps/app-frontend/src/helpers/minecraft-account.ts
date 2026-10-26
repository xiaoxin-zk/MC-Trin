import type { Skin } from './skins'

export interface MinecraftCredential {
    account_id: string
    account_type: 'microsoft' | 'offline' | 'yggdrasil'
    profile: {
        id: string
        name: string
        skins?: Array<{
            state: string
            url: string
            variant: Skin['variant']
            textureKey?: string
        }>
    }
    yggdrasil?: {
        api_root: string
        server_name: string
        login: string
    }
}
