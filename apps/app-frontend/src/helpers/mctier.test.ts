import assert from 'node:assert/strict'
import test from 'node:test'

import { parseMctierInviteCode } from './mctier.ts'

test('parses a MCTier v3 invite and keeps the encrypted password envelope', () => {
    const invite = parseMctierInviteCode(
        'mctier://join?v=3&name=TestRoom&secret=mctier-invite-v3%3Aabc_DEF-123&node=tcp%3A%2F%2Fnode.example%3A11010&signal=wss%3A%2F%2Fsignal.example%2Fsignaling',
    )

    assert.deepEqual(invite, {
        lobby_name: 'TestRoom',
        password: 'mctier-invite-v3:abc_DEF-123',
        server_node: 'tcp://node.example:11010',
        signaling_server: 'wss://signal.example/signaling',
    })
})

test('parses invites generated with the registered MC-Trin deep-link scheme', () => {
    const invite = parseMctierInviteCode(
        'mctrin://join?v=3&name=TestRoom&secret=mctier-invite-v3%3Aabc_DEF-123',
    )

    assert.equal(invite?.lobby_name, 'TestRoom')
    assert.equal(invite?.password, 'mctier-invite-v3:abc_DEF-123')
})

test('rejects malformed invite codes and unsafe connection endpoints', () => {
    assert.equal(parseMctierInviteCode('mctier://join?v=9&name=TestRoom'), null)
    assert.equal(parseMctierInviteCode('mctier://join?v=3&name=TestRoom&secret=plaintext'), null)
    assert.equal(
        parseMctierInviteCode(
            'mctier://join?v=3&name=TestRoom&secret=&node=file%3A%2F%2F%2Ftmp&signal=wss%3A%2F%2Fsignal.example',
        ),
        null,
    )
    assert.equal(parseMctierInviteCode('not-an-invite'), null)
})
