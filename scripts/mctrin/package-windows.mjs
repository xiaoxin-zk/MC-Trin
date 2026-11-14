import fs from 'node:fs'
import path from 'node:path'
import { parseArgs } from 'node:util'

import {
    app,
    files,
    output,
    pnpmCommand,
    requireFile,
    requireProfile,
    root,
    run,
    sha256,
    version,
} from './desktop-build-utils.mjs'
import { verifyFrontendBuild } from './verify-frontend-build.mjs'

const { values } = parseArgs({
    options: {
        profile: { type: 'string', default: 'release' },
        output: { type: 'string', default: path.join(root, 'target/release-assets/windows') },
    },
})
if (process.platform !== 'win32') throw new Error('Windows packaging requires Windows')
const profile = values.profile
requireProfile(profile)
const source = path.join(root, 'target', profile)
const binary = requireFile(path.join(source, 'MC-Trin.exe'))
// The installer bundles the executable, and the executable embeds whatever
// `apps/app-frontend/dist` held when it was compiled. Comparing asset names
// here catches a binary built against another frontend before the installer is
// shipped - the failure that put an old `AIAgent-*.js` into a release.
verifyFrontendBuild({ head: output('git', ['rev-parse', 'HEAD']), binary })
const manifest = JSON.parse(fs.readFileSync(path.join(source, 'mctrin-build.json'), 'utf8'))
if (
    manifest.commit !== output('git', ['rev-parse', 'HEAD']) ||
    manifest.binarySha256 !== sha256(binary)
) {
    throw new Error('The packaged binary does not match its build manifest')
}
const initialHash = sha256(binary)
process.env.MCTRIN_INSTALLER_UI_PATH = requireFile(path.join(source, 'mctrin-installer-ui.exe'))
const destination = path.resolve(values.output)
fs.mkdirSync(destination, { recursive: true })
const target =
    profile === 'release' ? path.join(root, 'target') : path.join(root, 'target/ci-bundle')
process.env.CARGO_TARGET_DIR = target
const releaseDirectory = path.join(target, 'release')
fs.mkdirSync(releaseDirectory, { recursive: true })
if (profile !== 'release') fs.copyFileSync(binary, path.join(releaseDirectory, 'MC-Trin.exe'))
const bundleDirectory = path.join(releaseDirectory, 'bundle/nsis')

for (const variant of ['modern', 'native']) {
    if (fs.existsSync(bundleDirectory)) {
        for (const file of fs.readdirSync(bundleDirectory)) {
            if (/(-setup\.exe|\.sig|\.nsis\.zip)$/.test(file))
                fs.unlinkSync(path.join(bundleDirectory, file))
        }
    }
    const args = [
        'tauri',
        'bundle',
        '--ci',
        '--bundles',
        'nsis',
        '--config',
        'tauri-release.conf.json',
        '--config',
        `tauri-${variant}.conf.json`,
    ]
    if (profile === 'ci') args.push('--no-sign')
    run(pnpmCommand(), args, { cwd: app })
    const installers = fs.readdirSync(bundleDirectory).filter((file) => file.endsWith('-setup.exe'))
    if (installers.length !== 1) throw new Error(`Expected one ${variant} installer`)
    const installer = path.join(bundleDirectory, installers[0])
    const name = `MC-Trin_${version()}_x64_${variant === 'modern' ? 'modern' : 'nsis'}-setup.exe`
    fs.copyFileSync(installer, path.join(destination, name))
    if (variant === 'native' && fs.existsSync(`${installer}.sig`)) {
        fs.copyFileSync(`${installer}.sig`, path.join(destination, `${name}.sig`))
    }
}
if (sha256(binary) !== initialHash) throw new Error('Packaging changed the compiled application')
const portable = path.join(root, 'target', `portable-${profile}`, 'MC-Trin')
fs.mkdirSync(path.join(portable, '.MC-Trin'), { recursive: true })
fs.copyFileSync(binary, path.join(portable, 'MC-Trin.exe'))
requireFile(path.join(app, 'resources/seed-map/mctrin-cubiomes.dll'))
fs.cpSync(path.join(app, 'resources/seed-map'), path.join(portable, 'resources/seed-map'), {
    recursive: true,
})
fs.cpSync(
    path.join(app, 'resources/blockbench-skin'),
    path.join(portable, 'resources/blockbench-skin'),
    {
        recursive: true,
    },
)
run(
    'powershell.exe',
    [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        'Compress-Archive -LiteralPath $env:MCTRIN_PORTABLE_SOURCE -DestinationPath $env:MCTRIN_PORTABLE_ZIP -Force',
    ],
    {
        env: {
            ...process.env,
            MCTRIN_PORTABLE_SOURCE: portable,
            MCTRIN_PORTABLE_ZIP: path.join(destination, `MC-Trin_${version()}_x64_portable.zip`),
        },
    },
)
for (const file of files(destination).filter((file) => path.dirname(file) === destination))
    console.log(file)
