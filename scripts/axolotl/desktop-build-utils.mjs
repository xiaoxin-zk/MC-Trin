import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export const root = fileURLToPath(new URL('../..', import.meta.url))
export const app = path.join(root, 'apps/app')

export function createSpawnInvocation(
    command,
    args,
    platform = process.platform,
    comspec = process.env.ComSpec || 'cmd.exe',
) {
    if (platform !== 'win32' || !/(?:pnpm|npm|\.bat|\.cmd)$/i.test(command)) {
        return { command, args, shell: false, windowsVerbatimArguments: false }
    }

    const commandLine = [command, ...args]
        .map((argument) => {
            if (/["\r\n%&|<>^()!]/.test(argument))
                throw new Error('Unsupported Windows shell argument')
            return `"${argument}"`
        })
        .join(' ')

    return {
        command: comspec,
        args: ['/d', '/s', '/c', `"${commandLine}"`],
        shell: false,
        windowsVerbatimArguments: true,
    }
}

export function run(command, args, options = {}) {
    const invocation = createSpawnInvocation(command, args, process.platform, options.env?.ComSpec)
    const result = spawnSync(invocation.command, invocation.args, {
        cwd: root,
        stdio: 'inherit',
        ...options,
        shell: invocation.shell,
        windowsVerbatimArguments: invocation.windowsVerbatimArguments,
    })
    if (result.error) throw result.error
    if (result.status !== 0) {
        throw new Error(`${path.basename(command)} failed (${result.signal || result.status})`)
    }
}

export function output(command, args, cwd = root) {
    const result = spawnSync(command, args, {
        cwd,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'inherit'],
    })
    if (result.error) throw result.error
    if (result.status !== 0) throw new Error(`${command} failed (${result.status})`)
    return result.stdout.trim()
}

export function files(directory) {
    return fs
        .readdirSync(directory, { withFileTypes: true })
        .flatMap((entry) => {
            const file = path.join(directory, entry.name)
            if (entry.isSymbolicLink()) throw new Error(`Unexpected symbolic link: ${file}`)
            return entry.isDirectory() ? files(file) : [file]
        })
        .sort()
}

export function sha256(file) {
    return createHash('sha256').update(fs.readFileSync(file)).digest('hex')
}

export function version() {
    return JSON.parse(fs.readFileSync(path.join(root, 'apps/app-frontend/package.json'), 'utf8'))
        .version
}

export function requireProfile(profile) {
    if (!['release', 'ci'].includes(profile))
        throw new Error(`Unsupported build profile: ${profile}`)
    if (profile === 'release' && process.env.CARGO_PROFILE_RELEASE_LTO !== 'true') {
        throw new Error('Published builds require CARGO_PROFILE_RELEASE_LTO=true (fat LTO)')
    }
}

export function requireFile(file) {
    if (!fs.statSync(file).isFile()) throw new Error(`Missing build input: ${file}`)
    return file
}
