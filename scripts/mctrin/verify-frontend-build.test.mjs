import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

import {
    assertBinaryEmbeds,
    assertFrontendDist,
    assetFiles,
    FRONTEND_STAMP_FILE,
    referencedAssets,
    verifyFrontendBuild,
} from './verify-frontend-build.mjs'

const HEAD = 'a'.repeat(40)
const OTHER = 'b'.repeat(40)

function fixture({ stamp, assets = { 'assets/AIAgent-New.js': 'console.log(1)' } } = {}) {
    const dist = fs.mkdtempSync(path.join(os.tmpdir(), 'mctrin-dist-'))
    for (const [file, contents] of Object.entries(assets)) {
        fs.mkdirSync(path.dirname(path.join(dist, file)), { recursive: true })
        fs.writeFileSync(path.join(dist, file), contents)
    }
    const scripts = Object.keys(assets)
        .filter((file) => file.startsWith('assets/'))
        .map((file) => `<script type="module" src="/${file}"></script>`)
        .join('\n')
    fs.writeFileSync(path.join(dist, 'index.html'), `<!doctype html>\n${scripts}`)
    if (stamp !== undefined) {
        fs.writeFileSync(path.join(dist, FRONTEND_STAMP_FILE), JSON.stringify(stamp))
    }
    return dist
}

function stampFor(commit, assets) {
    return { schema: 1, commit, builtAt: '2026-01-01T00:00:00.000Z', assets }
}

test('reads asset references from the built index.html', () => {
    assert.deepEqual(
        referencedAssets(
            '<script type="module" crossorigin src="/assets/index-abc.js"></script>' +
                '<link rel="stylesheet" href="/assets/index-def.css">' +
                '<link rel="icon" href="/mctrin.svg">',
        ),
        ['assets/index-abc.js', 'assets/index-def.css'],
    )
})

test('lists nested asset files relative to dist', () => {
    const dist = fixture({ assets: { 'assets/a.js': 'a', 'assets/nested/b.js': 'b' } })
    assert.deepEqual(assetFiles(dist), ['assets/a.js', 'assets/nested/b.js'])
})

test('accepts a dist whose stamp matches HEAD and whose files match the stamp', () => {
    const dist = fixture({ stamp: stampFor(HEAD, ['assets/AIAgent-New.js']) })
    const result = verifyFrontendBuild({ dist, head: HEAD })
    assert.equal(result.commit, HEAD)
    assert.deepEqual(result.assets, ['assets/AIAgent-New.js'])
})

test('a handwritten dist without a stamp is refused', () => {
    const dist = fixture()
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD }),
        /has no mctrin-frontend-build\.json/,
    )
})

test('a dist built from an older commit is refused when the frontend changed', () => {
    const dist = fixture({ stamp: stampFor(OTHER, ['assets/AIAgent-New.js']) })
    assert.throws(
        () =>
            assertFrontendDist({
                dist,
                head: HEAD,
                stamp: stampFor(OTHER, ['assets/AIAgent-New.js']),
                inputs: 'changed',
            }),
        /was made from bbbbbbbbbbbb, but HEAD is aaaaaaaaaaaa/,
    )
})

test('an older dist whose frontend inputs are identical is still accepted', () => {
    // A Turbo cache hit restores the previous build, whose assets are exactly
    // what a rebuild would emit; refusing it would only force a rebuild.
    const dist = fixture({ stamp: stampFor(OTHER, ['assets/AIAgent-New.js']) })
    assert.doesNotThrow(() =>
        assertFrontendDist({
            dist,
            head: HEAD,
            stamp: stampFor(OTHER, ['assets/AIAgent-New.js']),
            inputs: 'unchanged',
        }),
    )
})

test('an asset that the stamp does not list is refused', () => {
    const dist = fixture({
        stamp: stampFor(HEAD, ['assets/AIAgent-New.js']),
        assets: { 'assets/AIAgent-New.js': 'new', 'assets/AIAgent-Stale.js': 'stale' },
    })
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD }),
        /does not list: assets\/AIAgent-Stale\.js/,
    )
})

test('a stamped asset missing from disk is refused', () => {
    const dist = fixture({
        stamp: stampFor(HEAD, ['assets/AIAgent-New.js', 'assets/AIAgent-Gone.js']),
    })
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD }),
        /missing stamped files: assets\/AIAgent-Gone\.js/,
    )
})

test('an index.html that references a missing asset is refused', () => {
    const dist = fixture({ stamp: stampFor(HEAD, ['assets/AIAgent-New.js']) })
    fs.writeFileSync(
        path.join(dist, 'index.html'),
        '<script type="module" src="/assets/AIAgent-Missing.js"></script>',
    )
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD }),
        /references missing asset assets\/AIAgent-Missing\.js/,
    )
})

test('an executable that does not embed the current assets is refused', () => {
    const dist = fixture({ stamp: stampFor(HEAD, ['assets/AIAgent-New.js']) })
    const binary = path.join(dist, 'MC-Trin.exe')
    fs.writeFileSync(binary, 'blob with assets/AIAgent-Old.js inside')
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD, binary }),
        /does not embed the current frontend build/,
    )
    fs.writeFileSync(binary, 'blob with assets/AIAgent-New.js inside')
    assert.doesNotThrow(() => verifyFrontendBuild({ dist, head: HEAD, binary }))
})

test('an executable that embeds almost none of the assets is refused', () => {
    const assets = Object.fromEntries(
        Array.from({ length: 10 }, (_, index) => [`assets/chunk-${index}.js`, 'x']),
    )
    const dist = fixture({ stamp: stampFor(HEAD, Object.keys(assets)), assets })
    // The page loads only the entry chunk; the other nine are lazy chunks.
    fs.writeFileSync(
        path.join(dist, 'index.html'),
        '<script type="module" src="/assets/chunk-0.js"></script>',
    )
    const binary = path.join(dist, 'MC-Trin.exe')
    // The entry file the page loads is present, but the bulk of the bundle was
    // compiled from another build; most lazy chunk names cannot be found.
    fs.writeFileSync(binary, 'assets/chunk-0.js assets/chunk-1.js')
    assert.throws(
        () => verifyFrontendBuild({ dist, head: HEAD, binary }),
        /embeds only 2 of 10 assets/,
    )
})

test('a missing stamp or executable fails closed', () => {
    const dist = fixture({ stamp: stampFor(OTHER, ['assets/AIAgent-New.js']) })
    assert.throws(() => assertFrontendDist({ dist, head: HEAD, stamp: null }))
    assert.throws(() =>
        assertBinaryEmbeds(
            dist,
            { referenced: ['assets/AIAgent-New.js'], assets: ['assets/AIAgent-New.js'] },
            path.join(dist, 'nope.exe'),
        ),
    )
})
