// This tag was published only for temporary testing and is not a release baseline.
export function isExcludedRelease(tag) {
    return tag === 'v1.9.9'
}

export function selectLatestStableRelease(releases) {
    return releases.find(
        (release) => !release.draft && !release.prerelease && !isExcludedRelease(release.tag_name),
    )
}

// New release tags must exceed this baseline: the historical 0.0.x tag series
// numbers below the repository metadata series (1.10.x), so a lower tag would
// make update checks treat installed 1.10.x launchers as permanently newer.
export const MINIMUM_RELEASE_TAG = 'v1.10.1-beta.1'

function parseVersion(tag) {
    const match = /^v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(tag)
    if (!match) return null
    return {
        core: [Number(match[1]), Number(match[2]), Number(match[3])],
        prerelease: match[4] ? match[4].split('.') : null,
    }
}

function compareVersions(left, right) {
    for (let index = 0; index < left.core.length; index++) {
        if (left.core[index] !== right.core[index]) return left.core[index] - right.core[index]
    }
    if (!left.prerelease || !right.prerelease) {
        // A stable release outranks its own prereleases.
        return left.prerelease ? -1 : right.prerelease ? 1 : 0
    }
    const length = Math.max(left.prerelease.length, right.prerelease.length)
    for (let index = 0; index < length; index++) {
        const x = left.prerelease[index]
        const y = right.prerelease[index]
        if (x === y) continue
        if (x === undefined) return -1
        if (y === undefined) return 1
        const numericX = /^\d+$/.test(x)
        const numericY = /^\d+$/.test(y)
        // SemVer: numeric identifiers always compare lower than alphanumeric ones.
        if (numericX !== numericY) return numericX ? -1 : 1
        if (numericX) return Number(x) - Number(y)
        return x < y ? -1 : 1
    }
    return 0
}

export function isAtOrBelowReleaseBaseline(tag) {
    const version = parseVersion(tag)
    // Unparseable tags fail closed: releaseVersion() validates the format first.
    return !version || compareVersions(version, parseVersion(MINIMUM_RELEASE_TAG)) <= 0
}
