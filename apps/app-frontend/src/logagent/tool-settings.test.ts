import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    ASSISTANT_WRITE_TOOLS,
    BUILT_IN_AGENT_TOOLS,
    DANGEROUS_AGENT_TOOLS,
    toolNeedsApproval,
} from './tool-settings.ts'

const EXPECTED_WRITE_TOOLS = [
    'update_launcher_settings',
    'create_instance',
    'rename_instance',
    'delete_instance',
    'install_java_runtime',
    'set_java_default_runtime',
    'queue_resource_install',
    'cancel_install_job',
    'disable_content',
    'enable_content',
    'remove_content',
]

test('the renderer write list is exactly the eleven gated tools', () => {
    // The same list is compared with `assistant_write_tool` in
    // `apps/app/tests/logagent_write_tool_parity.rs`; this copy makes a local
    // `test:unit` run catch an accidental edit without compiling Rust.
    assert.deepEqual([...ASSISTANT_WRITE_TOOLS].sort(), [...EXPECTED_WRITE_TOOLS].sort())
})

test('every write tool is declared as a write tool in the tool settings', () => {
    for (const id of ASSISTANT_WRITE_TOOLS) {
        const definition = BUILT_IN_AGENT_TOOLS.find((tool) => tool.id === id)
        assert.ok(definition, `${id} is gated but missing from the tool list`)
        assert.equal(definition.permission, 'write', `${id} must be declared as a write`)
    }
})

test('the danger set does not hide a write tool from the approval prompt', () => {
    // `free` mode auto-approves everything the danger set does not list, so the
    // set must contain every write tool.
    for (const id of ASSISTANT_WRITE_TOOLS) {
        assert.ok(DANGEROUS_AGENT_TOOLS.has(id), `${id} would run unapproved in free mode`)
    }
})

test('free mode keeps read tools prompt-free but asks for every write', () => {
    for (const tool of BUILT_IN_AGENT_TOOLS) {
        const expected = tool.permission === 'write'
        assert.equal(
            toolNeedsApproval('free', tool.id, true),
            expected,
            `free mode ${expected ? 'must' : 'must not'} prompt for ${tool.id}`,
        )
    }
})

test('remote tools ask in every mode except full access', () => {
    for (const mode of ['default', 'free'] as const) {
        assert.equal(toolNeedsApproval(mode, 'lookup_server', false), true)
    }
    assert.equal(toolNeedsApproval('full', 'lookup_server', false), false)
})

test('default mode asks for every tool and full access asks for none', () => {
    assert.equal(toolNeedsApproval('default', 'get_instance_overview', true), true)
    assert.equal(toolNeedsApproval('default', 'create_instance', true), true)
    assert.equal(toolNeedsApproval('full', 'create_instance', true), false)
})
