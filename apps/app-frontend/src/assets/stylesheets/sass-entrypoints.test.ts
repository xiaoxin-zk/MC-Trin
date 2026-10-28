import assert from 'node:assert/strict'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import * as sass from 'sass'

const repositoryRoot = new URL('../../../../../', import.meta.url)
const assetsRoot = new URL('packages/assets/', repositoryRoot)

function compileStylesheet(relativePath: string): string {
    const deprecations: string[] = []
    const result = sass.compile(fileURLToPath(new URL(relativePath, repositoryRoot)), {
        fatalDeprecations: ['import'],
        importers: [
            {
                findFileUrl(url) {
                    const prefix = '@modrinth/assets/'
                    return url.startsWith(prefix)
                        ? new URL(url.slice(prefix.length), assetsRoot)
                        : null
                },
            },
        ],
        logger: {
            warn(message, options) {
                if (options.deprecation) deprecations.push(message)
            },
        },
    })
    assert.deepEqual(deprecations, [], `${relativePath} emitted Sass deprecation warnings`)
    return result.css
}

for (const relativePath of [
    'packages/assets/omorphia.scss',
    'apps/app-frontend/src/test/visual-styles.scss',
    'packages/ui/src/test/visual-styles.scss',
]) {
    test(`${relativePath} compiles without Sass deprecations`, () => {
        assert.ok(compileStylesheet(relativePath).includes('--surface-1:'))
    })
}

test('desktop styles keep the Tailwind, launcher and shared style cascade', () => {
    const css = compileStylesheet('apps/app-frontend/src/assets/stylesheets/global.scss')
    const positions = [
        '@tailwind utilities;',
        '--dc-app-bg:',
        '.visually-hidden',
        '.light-properties',
        '--color-green-400: #66e6a0;',
    ].map((part) => {
        const position = css.indexOf(part)
        assert.notEqual(position, -1, `missing expected stylesheet output: ${part}`)
        return position
    })
    assert.deepEqual(
        positions,
        [...positions].sort((left, right) => left - right),
    )
    assert.match(css, /html\.dark-mode,\s*html\.oled-mode\s*\{/)
})
