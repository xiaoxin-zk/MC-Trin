import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

const CRATES = [
    { root: 'apps/app', package: 'theseus_gui', features: ['updater'] },
    { root: 'apps/installer-ui', package: 'axolotl-installer-ui' },
    { root: 'packages/app-lib', package: 'theseus', moduleTests: true },
    { root: 'packages/content-management', package: 'content-management' },
    { root: 'packages/daedalus', package: 'daedalus' },
    { root: 'packages/path-util', package: 'path-util' },
    {
        root: 'packages/async-minecraft-ping',
        manifest: 'packages/async-minecraft-ping/Cargo.toml',
    },
]

const WORKSPACE_FILES = /^(?:Cargo\.(?:toml|lock)|rust-toolchain\.toml|\.cargo\/.*)$/u
const RUST_SOURCE_FILES = /(?:^|\/)(?:build\.rs|[^/]+\.rs|Cargo\.toml)$/u

export function moduleFilter(relativePath) {
    const match = /^packages\/app-lib\/src\/(.+)\.rs$/.exec(relativePath)
    if (!match) return null
    let module = match[1]
    module = module.replace(/(^|\/)mod$/u, '')
    module = module.replace(/(^|\/)tests$/u, '')
    if (!module || module === 'lib') return null
    return module.replace(/\//gu, '::')
}

function changedFiles(base) {
    if (!base || /^0+$/u.test(base)) return null
    try {
        const resolved = execFileSync('git', ['rev-parse', '--verify', `${base}^{commit}`], {
            encoding: 'utf8',
            timeout: 30000,
        }).trim()
        const output = execFileSync(
            'git',
            ['diff', '--name-only', '--no-renames', `${resolved}...HEAD`],
            { encoding: 'utf8', timeout: 30000 },
        )
        return output
            .split('\n')
            .map((line) => line.trim())
            .filter(Boolean)
    } catch {
        return null
    }
}

function crateForFile(file) {
    return CRATES.find((crate) => file.startsWith(`${crate.root}/`))
}

function workspaceCompilePlan(reason) {
    return { crates: CRATES, compileWorkspace: true, reason }
}

export function selectPlan(files) {
    if (!files) return workspaceCompilePlan('no baseline')
    if (files.some((file) => WORKSPACE_FILES.test(file))) {
        return workspaceCompilePlan('workspace-wide change')
    }

    const changed = new Map()
    let unknownRustChange = false
    for (const file of files) {
        if (!RUST_SOURCE_FILES.test(file)) continue
        const crate = crateForFile(file)
        if (!crate) {
            if (/^(?:apps|packages)\//u.test(file)) unknownRustChange = true
            continue
        }
        let entry = changed.get(crate.package)
        if (!entry) {
            entry = { ...crate, filters: new Set(), runWholePackage: false }
            changed.set(crate.package, entry)
        }

        const filter = crate.moduleTests ? moduleFilter(file) : null
        if (filter) entry.filters.add(filter)
        else entry.runWholePackage = true
    }

    if (unknownRustChange) return workspaceCompilePlan('unmapped Rust crate')
    return {
        crates: [...changed.values()].map((entry) => {
            const filters = [...entry.filters].filter(
                (filter) =>
                    ![...entry.filters].some(
                        (parent) => parent !== filter && filter.startsWith(`${parent}::`),
                    ),
            )
            return {
                ...entry,
                filters: entry.runWholePackage ? [] : filters.sort(),
            }
        }),
        compileWorkspace: false,
        reason: changed.size ? 'changed Rust crates' : 'no Rust change',
    }
}

export function targetedCargoArgs(plan) {
    if (plan.compileWorkspace) {
        return plan.crates.map((crate) => {
            const args = crate.manifest
                ? ['check', '--manifest-path', crate.manifest, '--all-targets']
                : ['check', '--package', crate.package, '--all-targets']
            if (crate.features) args.push('--features', crate.features.join(','))
            return args
        })
    }
    return plan.crates.flatMap((crate) => {
        const base = crate.manifest
            ? ['test', '--manifest-path', crate.manifest, '--no-fail-fast']
            : crate.moduleTests
              ? ['test', '--package', crate.package, '--lib', '--no-fail-fast']
              : ['test', '--package', crate.package, '--all-targets', '--no-fail-fast']
        if (crate.features) base.push('--features', crate.features.join(','))
        if (crate.filters.length > 0) {
            return crate.filters.map((filter) => [...base, filter, '--', '--test-threads=1'])
        }
        return [[...base, '--', '--test-threads=1']]
    })
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
    const plan = selectPlan(changedFiles(process.env.CHANGE_BASE))
    const commands = targetedCargoArgs(plan)
    if (commands.length === 0) {
        console.log(`Targeted tests: no Rust changes (${plan.reason})`)
        process.exit(0)
    }
    console.log(`Targeted tests: ${commands.length} command(s) (${plan.reason})`)
    for (const args of commands) {
        console.log(`cargo ${args.join(' ')}`)
        execFileSync('cargo', args, { stdio: 'inherit' })
    }
}
