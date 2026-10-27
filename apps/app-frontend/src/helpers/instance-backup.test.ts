import assert from 'node:assert/strict'
import test from 'node:test'

import {
    backupProgressState,
    canonicalizeBackupExclusions,
    formatBackupExclusionPath,
    isBackupActivityOperation,
} from './instance-backup.ts'

test('directory exclusions display with a trailing slash', () => {
    assert.equal(formatBackupExclusionPath({ path: 'caches', kind: 'directory' }), 'caches/')
    assert.equal(
        formatBackupExclusionPath({ path: 'config/somemod/bigdb.sqlite', kind: 'file' }),
        'config/somemod/bigdb.sqlite',
    )
})

test('directory exclusions cover descendants while file exclusions stay exact', () => {
    assert.deepEqual(
        canonicalizeBackupExclusions([
            { path: 'saves/world', kind: 'directory' },
            { path: 'config\\bigdb.sqlite', kind: 'file' },
            { path: 'saves', kind: 'directory' },
            { path: 'config', kind: 'file' },
        ]),
        [
            { path: 'config', kind: 'file' },
            { path: 'saves', kind: 'directory' },
            { path: 'config/bigdb.sqlite', kind: 'file' },
        ],
    )
})

test('directory wins when the same exclusion is selected as both kinds', () => {
    assert.deepEqual(
        canonicalizeBackupExclusions([
            { path: 'cache', kind: 'file' },
            { path: 'cache', kind: 'directory' },
        ]),
        [{ path: 'cache', kind: 'directory' }],
    )
})

test('backup activity includes deletion and maps its progress stage', () => {
    assert.equal(isBackupActivityOperation('delete'), true)
    assert.equal(backupProgressState('deleting'), 'deleting')
    assert.equal(backupProgressState('copying'), 'materializing')
})
