import assert from 'node:assert/strict'
import test from 'node:test'

import { createSpawnInvocation } from './desktop-build-utils.mjs'

test('Windows package manager shims avoid Node shell mode and quote command arguments', () => {
    const invocation = createSpawnInvocation(
        'pnpm',
        ['tauri', 'bundle', '--config', 'path with spaces', 'feature-flag'],
        'win32',
        'C:\\Windows\\System32\\cmd.exe',
    )

    assert.deepEqual(invocation, {
        command: 'C:\\Windows\\System32\\cmd.exe',
        args: [
            '/d',
            '/s',
            '/c',
            '""pnpm" "tauri" "bundle" "--config" "path with spaces" "feature-flag""',
        ],
        shell: false,
        windowsVerbatimArguments: true,
    })
})

test('Windows package manager shims reject cmd metacharacters and quote injection', () => {
    const unsafeArguments = [
        '%PATH%',
        '&',
        '|',
        '<',
        '>',
        '^',
        '(',
        ')',
        '!',
        'bad"argument',
        'line\nbreak',
    ]
    for (const argument of unsafeArguments) {
        assert.throws(
            () => createSpawnInvocation('npm', [argument], 'win32'),
            /Unsupported Windows shell argument/,
        )
    }
    assert.throws(
        () => createSpawnInvocation('pnpm&evil.cmd', [], 'win32'),
        /Unsupported Windows shell argument/,
    )
})

test('non-Windows commands retain direct arguments without enabling a shell', () => {
    const args = ['pnpm', 'arguments with spaces']
    assert.deepEqual(createSpawnInvocation('pnpm', args, 'linux'), {
        command: 'pnpm',
        args,
        shell: false,
        windowsVerbatimArguments: false,
    })
})
