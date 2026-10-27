import type { SaveGameSettingsResult } from './game-options'

export function hasIncompleteGameSettingsSave(result: SaveGameSettingsResult): boolean {
    return result.deferred > 0 || result.unsupported > 0 || result.failed > 0
}
