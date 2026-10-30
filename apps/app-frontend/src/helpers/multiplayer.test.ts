import assert from 'node:assert/strict'
import test from 'node:test'

import {
    type DetectedLanPort,
    type HongshiNode,
    multiplayerErrorText,
    sanitizeErrorText,
    selectedDetectedInstance,
    selectedNodePreference,
    storedMultiplayerProvider,
    validLocalPort,
} from './multiplayer.ts'

const detectedPort = (instanceId: string): DetectedLanPort => ({
    instance_id: instanceId,
    instance_name: `Instance ${instanceId}`,
    process_id: `process-${instanceId}`,
    port: 25565,
    detected_at: '2026-08-12 12:00:00',
})

const node = (name: string): HongshiNode => ({
    name,
    address: '203.0.113.1',
    latency_ms: 20,
    reachable: true,
    cached: false,
})

test('defaults the provider preference to Terracotta', () => {
    assert.equal(storedMultiplayerProvider(null), 'mctier')
    assert.equal(storedMultiplayerProvider('invalid'), 'mctier')
    assert.equal(storedMultiplayerProvider('hongshi'), 'hongshi')
})

test('accepts only complete ports in the Minecraft port range', () => {
    assert.equal(validLocalPort('1'), 1)
    assert.equal(validLocalPort('65535'), 65535)
    for (const value of ['', '0', '65536', '25565abc', ' 25565']) {
        assert.equal(validLocalPort(value), null)
    }
})

test('auto-selects exactly one detected instance and preserves valid choices', () => {
    assert.equal(selectedDetectedInstance('manual', [detectedPort('a')]), 'a')
    assert.equal(selectedDetectedInstance('a', [detectedPort('a'), detectedPort('b')]), 'a')
    assert.equal(
        selectedDetectedInstance('missing', [detectedPort('a'), detectedPort('b')]),
        'manual',
    )
})

test('falls back to automatic node selection when a cached preference disappears', () => {
    assert.equal(selectedNodePreference('Nanjing', [node('Nanjing')]), 'Nanjing')
    assert.equal(selectedNodePreference('Missing', [node('Nanjing')]), 'auto')
})

test('reads cancellation messages from serialized Tauri errors', () => {
    assert.equal(
        multiplayerErrorText({ field_name: 'Theseus', message: 'RedStone operation cancelled' }),
        'RedStone operation cancelled',
    )
    assert.equal(
        multiplayerErrorText('RedStone operation cancelled'),
        'RedStone operation cancelled',
    )
})

test('keeps internal Rust diagnostics out of user-facing errors', () => {
    // The lobby bug surfaced as a rustc source path in the dialog.
    assert.equal(
        sanitizeErrorText(
            'the signaling server did not confirm lobby visibility\n\nLocation: /rustc/59807616e1fa2540724bfbac14d7976d7e4a3860/library/core/src/ops/function.rs:250:5',
        ),
        'the signaling server did not confirm lobby visibility',
    )
    assert.equal(
        sanitizeErrorText(
            'the room session is no longer active\n\nStack backtrace:\n   0: std::backtrace_rs::backtrace::libunwind::trace',
        ),
        'the room session is no longer active',
    )
    assert.equal(
        sanitizeErrorText(
            'EasyTier network process exited: status 1\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace',
        ),
        'EasyTier network process exited: status 1',
    )
    // Actionable text survives untouched.
    assert.equal(
        sanitizeErrorText('Only the room host can change public visibility'),
        'Only the room host can change public visibility',
    )
})
