import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

import type { Plugin } from 'vite'

const STAMP_FILE = 'mctrin-frontend-build.json'

function commitHash(): string {
    // A source export without `.git` records an empty commit, and packaging then
    // refuses the result rather than shipping a frontend of unknown origin.
    try {
        return execFileSync('git', ['rev-parse', 'HEAD'], {
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
        }).trim()
    } catch {
        return ''
    }
}

/** Every emitted asset file, relative to `dist`, sorted. */
function assetFiles(directory: string): string[] {
    const files: string[] = []
    function walk(current: string) {
        if (!fs.existsSync(current)) return
        for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
            const file = path.join(current, entry.name)
            if (entry.isDirectory()) walk(file)
            else if (entry.isFile()) {
                files.push(path.relative(directory, file).split(path.sep).join('/'))
            }
        }
    }
    walk(path.join(directory, 'assets'))
    return files.sort()
}

/**
 * Writes a build stamp beside the produced assets.
 *
 * `apps/app-frontend/dist` is git-ignored and Tauri embeds it wholesale, so an
 * old directory silently becomes the shipped frontend. Recording the commit
 * and the emitted asset names lets `scripts/mctrin/verify-frontend-build.mjs`
 * refuse to package a `dist` that does not belong to `HEAD`.
 */
export function frontendBuildStamp(): Plugin {
    let outDir = ''
    return {
        name: 'mctrin-frontend-build-stamp',
        apply: 'build',
        configResolved(config) {
            outDir = path.resolve(config.root, config.build.outDir)
        },
        closeBundle() {
            if (!outDir || !fs.existsSync(outDir)) return
            const stamp = {
                schema: 1,
                commit: commitHash(),
                builtAt: new Date().toISOString(),
                assets: assetFiles(outDir),
            }
            fs.writeFileSync(path.join(outDir, STAMP_FILE), `${JSON.stringify(stamp, null, 2)}\n`)
        },
    }
}
