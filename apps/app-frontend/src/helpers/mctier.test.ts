import assert from 'node:assert/strict'
import test from 'node:test'

import {
    DEFAULT_MCTIER_NODE,
    DEFAULT_MCTIER_SIGNALING,
    isValidDirectServerNode,
    isValidMctierLobbyName,
    isValidMctierPlayerName,
    isValidMctierSignalingServer,
    parseMctierInviteCode,
} from './mctier.ts'

/**
 * The shipped defaults are what every fresh room form sends. A validator that
 * rejects them makes room creation impossible before any network work begins,
 * and the form only greys out its buttons, so the failure looks like a dead UI.
 * This must never regress.
 */
test('the shipped node and signalling defaults satisfy their own validators', () => {
    assert.equal(isValidDirectServerNode(DEFAULT_MCTIER_NODE), true)
    assert.equal(isValidMctierSignalingServer(DEFAULT_MCTIER_SIGNALING), true)
    const invite = parseMctierInviteCode(
        `mctier://join?v=3&name=TestRoom&secret=&node=${encodeURIComponent(
            DEFAULT_MCTIER_NODE,
        )}&signal=${encodeURIComponent(DEFAULT_MCTIER_SIGNALING)}`,
    )
    assert.equal(invite?.server_node, DEFAULT_MCTIER_NODE)
    assert.equal(invite?.signaling_server, DEFAULT_MCTIER_SIGNALING)
})

test('the node validator rejects every address the backend refuses', () => {
    for (const invalid of [
        '',
        'node.example:11010',
        'file:///etc/passwd',
        'tcp://user:pass@node.example:11010',
        'tcp://node.example:11010/path',
        'ftp://node.example:11010',
        // tcp/udp have no default port, so the backend needs it spelled out.
        'tcp://node.example',
        'udp://node.example',
        'tcp://node.example:11010?query=1',
        'tcp://node.example:11010#fragment',
        'tcp://host.example:11010?',
        'tcp://host.example:11010#',
        'tcp://host:11010?',
        'tcp://host:11010#',
        // A control character and an over-long address fail the shared
        // precheck before the URL is even parsed, exactly like Rust.
        'tcp://node.example:11010\u0000',
        'tcp://node.exa\nmple:11010',
        `tcp://${'a'.repeat(2100)}:11010`,
    ]) {
        assert.equal(isValidDirectServerNode(invalid), false, invalid)
    }
    // 2048 UTF-8 bytes is the limit. A multi-byte hostname spends more than one
    // byte per character, so the bound must count bytes rather than UTF-16 units.
    const host = 'a'.repeat(2040)
    assert.equal(isValidDirectServerNode(`tcp://${host}:11010`), false)
    const multiByteHost = '\u00e9'.repeat(1030)
    assert.equal(isValidDirectServerNode(`tcp://${multiByteHost}:11010`), false)
    for (const valid of [
        'tcp://node.example:11010',
        'udp://node.example:11010',
        'tcp://node.example:11010/',
        'ws://node.example',
        'wss://node.example',
        'ws://node.example:8080',
    ]) {
        assert.equal(isValidDirectServerNode(valid), true, valid)
    }
})

test('the signalling validator only accepts a secure remote or a loopback socket', () => {
    assert.equal(isValidMctierSignalingServer('ws://node.example:1234'), false)
    assert.equal(isValidMctierSignalingServer('http://node.example'), false)
    assert.equal(isValidMctierSignalingServer('wss://user:pass@node.example'), false)
    assert.equal(isValidMctierSignalingServer('wss://node.example/#fragment'), false)
    assert.equal(isValidMctierSignalingServer('wss://host.example/signaling#'), false)
    assert.equal(isValidMctierSignalingServer('wss://host/#'), false)
    // The service lives under a path, and query parameters are allowed there.
    assert.equal(isValidMctierSignalingServer('wss://host/signaling?token=x'), true)
    assert.equal(isValidMctierSignalingServer('wss://host.example/signaling?token=x'), true)
    assert.equal(isValidMctierSignalingServer('wss://host.example/signaling?'), true)
    assert.equal(isValidMctierSignalingServer('wss://node.example/signaling?token=abc'), true)
    assert.equal(isValidMctierSignalingServer('wss://host.example/signaling%23%3F'), true)
    assert.equal(isValidMctierSignalingServer('wss://host.example/signaling?token=%23%3F'), true)
    assert.equal(isValidMctierSignalingServer('ws://localhost:1234'), true)
    assert.equal(isValidMctierSignalingServer('ws://127.0.0.1:1234'), true)
    assert.equal(isValidMctierSignalingServer('ws://[::1]:1234'), true)
})

test('a lobby name needs at least one letter or number, like the backend', () => {
    // The backend rejects symbol-only names; the form used to accept them and
    // then fail with a generic error after the kernel was already starting.
    assert.equal(isValidMctierLobbyName('____'), false)
    assert.equal(isValidMctierLobbyName('----'), false)
    assert.equal(isValidMctierLobbyName('_-_-'), false)
    // A single letter or number anywhere is enough.
    assert.equal(isValidMctierLobbyName('___a'), true)
    assert.equal(isValidMctierLobbyName('mc-abc234'), true)
    assert.equal(isValidMctierLobbyName('房间一二'), true)

    // Length in code points, matching Rust's `chars().count()`.
    assert.equal(isValidMctierLobbyName('abc'), false)
    assert.equal(isValidMctierLobbyName('abcd'), true)
    assert.equal(isValidMctierLobbyName('a'.repeat(32)), true)
    assert.equal(isValidMctierLobbyName('a'.repeat(33)), false)
    // Surrounding whitespace and control characters are refused.
    assert.equal(isValidMctierLobbyName(' room'), false)
    assert.equal(isValidMctierLobbyName('room '), false)
    assert.equal(isValidMctierLobbyName('ro\nom'), false)
    assert.equal(isValidMctierLobbyName('ro om'), false)
})

test('the player name is limited to 64 characters without control characters', () => {
    assert.equal(isValidMctierPlayerName('Alice'), true)
    // The form submits a trimmed name, so surrounding spaces are not a failure.
    assert.equal(isValidMctierPlayerName('  Alice  '), true)
    assert.equal(isValidMctierPlayerName('   '), false)
    assert.equal(isValidMctierPlayerName('a'.repeat(64)), true)
    assert.equal(isValidMctierPlayerName('a'.repeat(65)), false)
    // Code points, not UTF-16 units: an emoji is one character to Rust.
    assert.equal(isValidMctierPlayerName('😀'.repeat(64)), true)
    assert.equal(isValidMctierPlayerName('Alic\te'), false)
    assert.equal(isValidMctierPlayerName('Alic\u0000e'), false)
})

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
