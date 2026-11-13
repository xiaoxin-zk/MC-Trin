import { readFile, readdir } from 'node:fs/promises'
import path from 'node:path'

/**
 * Onboarding steps point at components through `data-onboarding-id`. A step
 * whose target no longer exists silently times out and the tour skips it, so a
 * rewritten page can drop guidance without any visible failure. This guard
 * compares the targets the tour configuration references against the targets
 * the components actually render.
 */

const configPath = 'apps/app-frontend/src/components/ui/onboarding/onboardingConfig.ts'
// Shared UI lives in `packages/ui`; the creation flow's targets are rendered
// there, so scanning the app alone reports them as missing.
const sourceRoots = ['apps/app-frontend/src', 'packages/ui/src']
const sourceExtensions = new Set(['.vue', '.ts', '.tsx', '.js', '.mjs'])
const ignoredDirectories = new Set([
    'node_modules',
    '.vite',
    'dist',
    '.output',
    '__screenshots__',
    'public',
])

/** Every `targetId: 'name'` the tour configuration references. */
export function configuredTargets(source) {
    const targets = new Set()
    const pattern = /targetId:\s*'([a-z0-9-]+)'/gu
    for (const match of source.matchAll(pattern)) targets.add(match[1])
    // `inspect(stepId, targetId, title, description)` takes the target as its
    // second positional argument, which carries no key to match on.
    const inspectPattern = /\binspect\(\s*'[a-z0-9-]+',\s*'([a-z0-9-]+)'/gu
    for (const match of source.matchAll(inspectPattern)) targets.add(match[1])
    return targets
}

/** Targets that `branchByTarget` may reference by key. */
export function branchedTargets(source) {
    const targets = new Set()
    // Keys are quoted target ids; each maps to the step the tour jumps to.
    const pattern = /branchByTarget:\s*\{([\s\S]*?)\n\s*\}/gu
    for (const block of source.matchAll(pattern)) {
        for (const entry of block[1].matchAll(/'([a-z0-9-]+)'\s*:/gu)) {
            targets.add(entry[1])
        }
    }
    return targets
}

/** Every `data-onboarding-id="name"` a template renders literally. */
export function renderedTargets(source) {
    const targets = new Set()
    const pattern = /data-onboarding-id="([a-z0-9-]+)"/gu
    for (const match of source.matchAll(pattern)) targets.add(match[1])
    return targets
}

/**
 * Templates may bind the attribute to a per-item value
 * (`:data-onboarding-id="category.onboardingId"`), so the ids live in the data
 * that feeds the list. Collect `onboardingId: 'name'` declarations from any
 * module, which is where those lists are defined.
 */
export function declaredTargets(source) {
    const targets = new Set()
    const pattern = /\bonboardingId:\s*'([a-z0-9-]+)'/gu
    for (const match of source.matchAll(pattern)) targets.add(match[1])
    return targets
}

export function missingTargets(configured, rendered) {
    return [...configured].filter((target) => !rendered.has(target)).sort()
}

async function collectSources(directory, files) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
        if (entry.isDirectory()) {
            if (ignoredDirectories.has(entry.name)) continue
            await collectSources(path.join(directory, entry.name), files)
            continue
        }
        if (sourceExtensions.has(path.extname(entry.name))) {
            files.push(path.join(directory, entry.name))
        }
    }
    return files
}

async function main() {
    const config = await readFile(configPath, 'utf8')
    const configured = new Set([
        ...configuredTargets(config),
        ...branchedTargets(config),
    ])

    const rendered = new Set()
    for (const root of sourceRoots) {
        for (const file of await collectSources(root, [])) {
            // The configuration file itself names targets without rendering them.
            if (file.endsWith('onboardingConfig.ts')) continue
            const source = await readFile(file, 'utf8')
            for (const target of renderedTargets(source)) rendered.add(target)
            // Bound attributes take their id from the module that defines the list.
            for (const target of declaredTargets(source)) rendered.add(target)
        }
    }

    const missing = missingTargets(configured, rendered)
    if (missing.length > 0) {
        console.error(
            'Onboarding steps reference targets no component renders:\n' +
                missing.map((target) => `  - ${target}`).join('\n') +
                '\n\nAdd the matching data-onboarding-id to the owning component,' +
                ' or drop the step from onboardingConfig.ts.',
        )
        process.exit(1)
    }

    console.log(
        `Onboarding targets resolve: ${configured.size} referenced, ` +
            `${rendered.size} rendered.`,
    )
}

if (process.argv[1] && import.meta.url.endsWith(path.basename(process.argv[1]))) {
    await main()
}
