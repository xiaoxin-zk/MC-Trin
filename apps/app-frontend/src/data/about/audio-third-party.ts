/** Third-party audio components bundled with the launcher.
 *
 * Kept as data rather than prose in the component so the about panel can render
 * them as a list, and so a future `intl:extract` pass does not pick the names up
 * as translatable copy — these are proper nouns and licence identifiers.
 *
 * The full licence texts, including the RNNoise BSD-3-Clause notice, live in
 * `AUDIO_THIRD_PARTY_NOTICES.md` beside this file. */
export type AudioThirdPartyComponent = {
    /** Name shown to the user. */
    name: string
    /** SPDX identifier, shown verbatim. */
    licence: string
    /** Why the launcher ships it, in one short clause. */
    purpose: string
}

export const audioThirdPartyComponents: readonly AudioThirdPartyComponent[] = [
    {
        name: 'RNNoise',
        licence: 'BSD-3-Clause',
        purpose: 'Optional AI noise reduction in room voice',
    },
    {
        name: '@shiguredo/rnnoise-wasm',
        licence: 'Apache-2.0',
        purpose: 'SIMD WebAssembly build of RNNoise that runs in the worklet',
    },
    {
        name: '@sapphi-red/web-noise-suppressor',
        licence: 'MIT',
        purpose: 'Web Audio wrapper that runs RNNoise',
    },
]
