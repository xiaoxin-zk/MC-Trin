const DEFAULT_GRID_GAP = 12
const MIN_CARD_WIDTH = 160
const MAX_COLUMNS = 6

/**
 * Calculates skin columns from the list's measured width, rather than the
 * viewport width. The skin page is a split layout, so viewport breakpoints
 * overestimate the space available to the grid.
 */
export function skinColumnCountForWidth(width: number): number {
    if (!Number.isFinite(width) || width <= 0) return 3
    return Math.max(
        1,
        Math.min(
            MAX_COLUMNS,
            Math.floor((width + DEFAULT_GRID_GAP) / (MIN_CARD_WIDTH + DEFAULT_GRID_GAP)),
        ),
    )
}
