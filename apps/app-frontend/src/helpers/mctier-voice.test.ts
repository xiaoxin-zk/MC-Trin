import assert from 'node:assert/strict'
import test from 'node:test'

import { parseMctierVoiceSignal } from './mctier-voice.ts'

test('accepts bounded SDP and ICE messages addressed to the local player', () => {
    const from = 'a'.repeat(64)
    const to = 'b'.repeat(64)
    assert.deepEqual(
        parseMctierVoiceSignal(
            { type: 'offer', from, to, offer: { type: 'offer', sdp: 'v=0' } },
            to,
        ),
        { type: 'offer', from, to, offer: { type: 'offer', sdp: 'v=0' } },
    )
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'ice-candidate',
                from,
                to,
                candidate: { candidate: 'candidate:1 1 udp 1 10.126.126.3 5000 typ host' },
            },
            to,
        )?.type,
        'ice-candidate',
    )
})

test('rejects messages for other identities and malformed or oversized payloads', () => {
    const from = 'a'.repeat(64)
    const to = 'b'.repeat(64)
    assert.equal(
        parseMctierVoiceSignal(
            { type: 'offer', from, to: 'c'.repeat(64), offer: { type: 'offer', sdp: 'v=0' } },
            to,
        ),
        null,
    )
    assert.equal(
        parseMctierVoiceSignal({ type: 'answer', from, to, answer: { type: 'answer' } }, to),
        null,
    )
    assert.equal(
        parseMctierVoiceSignal(
            {
                type: 'ice-candidate',
                from,
                to,
                candidate: { candidate: 'x'.repeat(16 * 1024 + 1) },
            },
            to,
        ),
        null,
    )
    assert.equal(parseMctierVoiceSignal('not an object', to), null)
})
