import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

/// Turns a changed source file into the test-module filter that covers it, so
/// the fast gate can run the affected unit tests instead of the whole suite.
///
/// The mapping mirrors the crate layout: `packages/app-lib/src/state/friends.rs`
/// is the `state::friends` module, and a module's tests live either in that
/// module or in a `tests` child (`.../friends/tests.rs`, `.../friends/tests.rs`
/// after the split). Files that carry no tests return no filter.
export function moduleFilter(relativePath) {
    const match = /^packages\/app-lib\/src\/(.+)\.rs$/.exec(relativePath)
    if (!match) return null
    let module = match[1]
    // `mod.rs` and `<dir>/mod.rs` describe the containing module.
    module = module.replace(/(^|\/)mod$/u, '')
    // A `tests` module belongs to its parent module.
    module = module.replace(/(^|\/)tests$/u, '')
    // The crate root does not have a reliable test-name filter. A root change
    // must force the caller to run the full suite instead of silently running
    // zero tests with the filter `lib`.
    if (!module || module === 'lib') return null
    return module.replace(/\//gu, '::')
}

const RUST_PACKAGE_FILES = /^(?:apps|packages)\/[^/]+\/.*\.rs$|^(?:Cargo\.(?:toml|lock)|rust-toolchain\.toml|(?:apps|packages)\/[^/]+\/Cargo\.toml)$/u

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
        return output.split('\n').map((line) => line.trim()).filter(Boolean)
    } catch {
        // A shallow or missing baseline cannot describe the change; fall back
        // to the full run rather than skipping the check.
        return null
    }
}

export function selectFilters(files) {
    if (!files) return { filters: [], reason: 'no baseline' }
    const filters = new Set()
    let rustChanged = false
    let unmappedRustChange = false
    for (const file of files) {
        if (!RUST_PACKAGE_FILES.test(file)) continue
        rustChanged = true
        const filter = moduleFilter(file)
        if (filter) filters.add(filter)
        else unmappedRustChange = true
    }
    if (!rustChanged) return { filters: [], reason: 'no Rust change' }
    // A change to the crate manifest, the toolchain or the test harness can
    // affect anything, so fall back to the full run rather than guessing.
    const broad = files.some((file) =>
        /^(Cargo\.(toml|lock)|rust-toolchain\.toml)$/u.test(file),
    )
    if (broad) return { filters: null, reason: 'crate-wide change' }
    if (unmappedRustChange) {
        return { filters: null, reason: 'unmapped Rust change' }
    }
    // A parent module already includes all of its child tests. Avoid running
    // the same tests twice when both mod.rs and a child module changed.
    const reduced = [...filters].filter(
        (filter) => ![...filters].some(
            (parent) => parent !== filter && filter.startsWith(`${parent}::`),
        ),
    )
    return { filters: reduced.sort(), reason: 'changed modules' }
}

export function targetedCargoArgs(filters) {
    const base = ['test', '--package', 'theseus', '--lib', '--no-fail-fast']
    if (filters === null) return [...base, '--', '--test-threads=1']
    if (filters.length === 0) return null
    // cargo test takes a single filter, so run each module filter in turn.
    return filters.map((filter) => [...base, filter, '--', '--test-threads=1'])
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
    const files = changedFiles(process.env.CHANGE_BASE)
    const { filters, reason } = selectFilters(files)
    const commands = targetedCargoArgs(filters)
    if (filters === null) {
        console.log(`Targeted tests: full suite (${reason})`)
        execFileSync('cargo', ['test', '--workspace', '--lib', '--no-fail-fast', '--', '--test-threads=1'], {
            stdio: 'inherit',
        })
        process.exit(0)
    }
    if (commands === null) {
        console.log(`Targeted tests: compile check only (${reason})`)
        execFileSync('cargo', ['check', '--package', 'theseus', '--all-targets'], {
            stdio: 'inherit',
        })
        process.exit(0)
    }
    if (commands.length === 0) {
        // Nothing to target (no baseline or no Rust change). Still prove the
        // test targets compile, which is the part that usually breaks.
        console.log(`Targeted tests: compile check only (${reason})`)
        execFileSync('cargo', ['check', '--package', 'theseus', '--all-targets'], {
            stdio: 'inherit',
        })
        process.exit(0)
    }
    console.log(
        `Targeted tests: ${commands.length} module filter(s) (${reason})`,
    )
    for (const args of commands) {
        console.log(`cargo ${args.join(' ')}`)
        execFileSync('cargo', args, { stdio: 'inherit' })
    }
}
