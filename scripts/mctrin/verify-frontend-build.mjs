import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'

import { output, root } from './desktop-build-utils.mjs'

/** Everything whose change can alter the built frontend bundle. */
const FRONTEND_INPUTS = [
    'apps/app-frontend',
    'packages/ui',
    'packages/assets',
    'packages/api-client',
    'packages/tooling-config',
]

/**
 * Stamp the frontend Vite build writes beside the assets it emitted.
 *
 * `apps/app-frontend/dist` is git-ignored and `apps/app/tauri.conf.json` points
 * `frontendDist` at it, so a hand-made or partial directory silently becomes
 * the shipped frontend. The stamp records the commit and the exact asset names
 * of one build; packaging cross-checks both the directory and the executable
 * that is about to be shipped.
 */
export const FRONTEND_STAMP_FILE = 'mctrin-frontend-build.json'

/** Asset paths the built `index.html` loads, relative to `dist`. */
export function referencedAssets(html) {
    const references = new Set()
    for (const match of html.matchAll(/(?:src|href)="\/(assets\/[^"?#]+)"/g)) {
        references.add(match[1])
    }
    return [...references].sort()
}

/** Every file under `dist/assets`, relative to `dist`, sorted. */
export function assetFiles(dist) {
    const files = []
    function walk(directory) {
        if (!fs.existsSync(directory)) return
        for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
            const file = path.join(directory, entry.name)
            if (entry.isDirectory()) walk(file)
            else if (entry.isFile()) {
                files.push(path.relative(dist, file).split(path.sep).join('/'))
            }
        }
    }
    walk(path.join(dist, 'assets'))
    return files.sort()
}

function shortCommit(commit) {
    return commit ? commit.slice(0, 12) : '<unknown>'
}

/**
 * Whether the frontend inputs differ between two commits.
 *
 * Returns `'unknown'` when git cannot answer - a shallow CI clone or a stamp
 * from a commit this checkout does not have. The caller only fails on
 * `'changed'`: the stamp is a debugging aid, and the check that actually
 * protects a release is the executable one below.
 */
export function frontendInputsChanged(from, to) {
    if (!from || !to) return 'unknown'
    const result = spawnSync(
        'git',
        ['diff', '--quiet', '--end-of-options', from, to, '--', ...FRONTEND_INPUTS],
        { cwd: root, stdio: 'ignore' },
    )
    if (result.status === 0) return 'unchanged'
    if (result.status === 1) return 'changed'
    return 'unknown'
}

/**
 * Validates one `dist` directory. Every message names the stale part so the
 * failing packaging step says what to rebuild instead of needing a bisect.
 */
export function assertFrontendDist({ dist, head, stamp, inputs = 'unknown' }) {
    if (!stamp) {
        throw new Error(
            `The frontend build in ${dist} has no ${FRONTEND_STAMP_FILE}. ` +
                'It was not produced by the current Vite build; run ' +
                '`pnpm turbo run build --filter=@modrinth/app-frontend` before packaging.',
        )
    }
    if (stamp.schema !== 1) {
        throw new Error(`Unsupported frontend build stamp schema in ${dist}: ${stamp.schema}`)
    }
    if (!stamp.commit) {
        throw new Error(`The frontend build in ${dist} does not record the commit it was made from`)
    }
    if (stamp.commit !== head && inputs === 'changed') {
        throw new Error(
            `The frontend build in ${dist} was made from ${shortCommit(stamp.commit)}, ` +
                `but HEAD is ${shortCommit(head)} and the frontend changed since. ` +
                'Rebuild it before packaging.',
        )
    }

    const index = path.join(dist, 'index.html')
    if (!fs.existsSync(index)) {
        throw new Error(`The frontend build in ${dist} has no index.html`)
    }
    const referenced = referencedAssets(fs.readFileSync(index, 'utf8'))
    if (referenced.length === 0) {
        throw new Error(`The frontend build in ${dist} references no assets; it is not a build`)
    }
    for (const asset of referenced) {
        if (!fs.existsSync(path.join(dist, asset))) {
            throw new Error(`The frontend build in ${dist} references missing asset ${asset}`)
        }
    }

    if (!Array.isArray(stamp.assets) || stamp.assets.length === 0) {
        throw new Error(`The frontend build stamp in ${dist} lists no assets`)
    }
    const stamped = [...new Set(stamp.assets)].sort()
    const onDisk = assetFiles(dist)
    const unstamped = onDisk.filter((file) => !stamped.includes(file))
    if (unstamped.length > 0) {
        throw new Error(
            `The frontend build in ${dist} contains files its stamp does not list: ` +
                `${unstamped.slice(0, 5).join(', ')}`,
        )
    }
    const missing = stamped.filter((file) => !onDisk.includes(file))
    if (missing.length > 0) {
        throw new Error(
            `The frontend build in ${dist} is missing stamped files: ` +
                `${missing.slice(0, 5).join(', ')}`,
        )
    }
    return { referenced, assets: stamped }
}

/**
 * Confirms the executable embeds the frontend build that is about to ship.
 *
 * Tauri records every asset key as a `&str` literal in the binary - see
 * `EmbeddedAssets::to_tokens` in `tauri-codegen`, which emits
 * `"assets/index-*.js" => { include_bytes!(..) }` - even though the file
 * contents are compressed. A key that is absent therefore proves the
 * executable was compiled against a different frontend build. That is how a
 * shipped installer ended up with an older `AIAgent-*.js` than the `dist`
 * beside it, and no assertion caught it.
 *
 * The scan stays on bytes: the keys are embedded as raw UTF-8, so decoding the
 * executable as `latin1` (one character per byte) turned `篝火-3KaXPIat.png`
 * into `ç¯ç«-3KaXPIat.png` and made every non-ASCII asset look missing.
 */
export function assertBinaryEmbeds(dist, build, binary) {
    if (!fs.existsSync(binary)) throw new Error(`Missing build artifact: ${binary}`)
    const contents = fs.readFileSync(binary)
    const embedded = (asset) => contents.includes(Buffer.from(path.basename(asset), 'utf8'))
    const missing = build.assets.filter((asset) => !embedded(asset))
    if (missing.length === 0) return
    if (missing.length === build.assets.length) {
        throw new Error(
            `${binary} contains none of the ${build.assets.length} asset names from ` +
                `${dist}. Either it was built from another frontend or it is not the ` +
                'application binary. Rebuild the application.',
        )
    }
    throw new Error(
        `${binary} does not embed the current frontend build in ${dist}: ` +
            `${missing.slice(0, 5).join(', ')} missing` +
            `${missing.length > 5 ? ` and ${missing.length - 5} more` : ''}. ` +
            'Rebuild the application.',
    )
}

export function verifyFrontendBuild({ dist, head, binary } = {}) {
    const directory = dist ?? path.join(root, 'apps/app-frontend/dist')
    const stampPath = path.join(directory, FRONTEND_STAMP_FILE)
    const stamp = fs.existsSync(stampPath) ? JSON.parse(fs.readFileSync(stampPath, 'utf8')) : null
    const inputs =
        stamp?.commit && stamp.commit !== head
            ? frontendInputsChanged(stamp.commit, head)
            : 'unchanged'
    const build = assertFrontendDist({ dist: directory, head, stamp, inputs })
    if (binary) assertBinaryEmbeds(directory, build, binary)
    return { dist: directory, commit: stamp.commit, assets: build.assets }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
    const { values } = parseArgs({
        options: {
            dist: { type: 'string' },
            binary: { type: 'string' },
            head: { type: 'string' },
        },
    })
    const result = verifyFrontendBuild({
        dist: values.dist ? path.resolve(values.dist) : undefined,
        head: values.head ?? output('git', ['rev-parse', 'HEAD']),
        binary: values.binary ? path.resolve(values.binary) : undefined,
    })
    console.log(
        `Frontend build ${shortCommit(result.commit)} verified (${result.assets.length} assets).`,
    )
}
