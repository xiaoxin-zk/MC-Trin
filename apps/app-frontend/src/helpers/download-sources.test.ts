import assert from 'node:assert/strict'
import test from 'node:test'

import {
    type CustomDownloadSource,
    normalizeDownloadSourceUrl,
    orderDownloadSources,
} from './download-sources.ts'

function source(id: string, priority: number): CustomDownloadSource {
    return {
        id,
        name: id,
        kind: 'modrinth',
        base_url: `https://${id}.example`,
        enabled: true,
        priority,
    }
}

test('normalizes HTTPS mirror base URLs', () => {
    assert.equal(
        normalizeDownloadSourceUrl(' https://mirror.example/modrinth/// '),
        'https://mirror.example/modrinth',
    )
})

test('rejects unsafe mirror URLs', () => {
    for (const url of [
        'http://mirror.example',
        'ftp://mirror.example/files',
        'https://user:pass@mirror.example',
        'https://mirror.example/path?token=secret',
        'https://mirror.example/path#fragment',
        'not a URL',
    ]) {
        assert.throws(() => normalizeDownloadSourceUrl(url), url)
    }
})

test('orders mirrors by priority and normalizes their positions', () => {
    const ordered = orderDownloadSources([
        source('third', 8),
        source('first', 0),
        source('second', 3),
    ])
    assert.deepEqual(
        ordered.map(({ id, priority }) => [id, priority]),
        [
            ['first', 0],
            ['second', 1],
            ['third', 2],
        ],
    )
})
