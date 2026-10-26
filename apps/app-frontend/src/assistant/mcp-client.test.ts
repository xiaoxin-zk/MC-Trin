import assert from 'node:assert/strict'
import test from 'node:test'

import { StreamableHttpMcpClient, type McpFetch } from './mcp-client.ts'

const endpoint = 'https://mcp.example.test/api/mcp'

function jsonResponse(id: number, result: unknown, headers?: HeadersInit) {
    return new Response(JSON.stringify({ jsonrpc: '2.0', id, result }), {
        headers: {
            'content-type': 'application/json',
            ...Object.fromEntries(new Headers(headers)),
        },
    })
}

test('initializes, lists tools, carries session headers, and calls only listed tools', async () => {
    const requests: Array<{ body: Record<string, unknown>; headers: Headers }> = []
    const fetcher: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as Record<string, unknown>
        const headers = new Headers(init.headers)
        requests.push({ body, headers })
        if (body.method === 'initialize')
            return jsonResponse(
                1,
                { protocolVersion: '2025-03-26' },
                { 'Mcp-Session-Id': 'session-1' },
            )
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        if (body.method === 'tools/list')
            return jsonResponse(2, {
                tools: [
                    {
                        name: 'search_logs',
                        description: 'Find matching log lines',
                        inputSchema: { type: 'object', properties: { query: { type: 'string' } } },
                    },
                ],
            })
        return jsonResponse(3, { content: [{ type: 'text', text: 'found it' }] })
    }
    const client = new StreamableHttpMcpClient(endpoint, 'secret-token', fetcher)

    const tools = await client.connect()
    const result = await client.callTool('search_logs', { query: 'error' })

    assert.deepEqual(
        tools.map((tool) => tool.name),
        ['search_logs'],
    )
    assert.deepEqual(result, { content: [{ type: 'text', text: 'found it' }] })
    assert.equal(requests[0].headers.get('authorization'), 'Bearer secret-token')
    assert.equal(requests[0].headers.get('mcp-session-id'), null)
    assert.equal(requests[1].headers.get('mcp-session-id'), 'session-1')
    assert.equal(requests[1].headers.get('mcp-protocol-version'), '2025-03-26')
    assert.equal(requests[2].body.method, 'tools/list')
    assert.equal(requests[3].body.method, 'tools/call')
    assert.equal(requests[3].headers.get('mcp-session-id'), 'session-1')
    assert.deepEqual(requests[3].body.params, {
        name: 'search_logs',
        arguments: { query: 'error' },
    })
    await assert.rejects(
        client.callTool('unlisted_tool', {}),
        /not in the connected server tool list/,
    )
    assert.equal(requests.length, 4)
})

test('accepts a matching JSON-RPC result from a server-sent event stream', async () => {
    const fetcher: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as Record<string, unknown>
        if (body.method === 'initialize') return jsonResponse(1, { protocolVersion: '2025-03-26' })
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        return new Response(
            `event: message\ndata: ${JSON.stringify({ jsonrpc: '2.0', id: 2, result: { tools: [] } })}\n\n`,
            { headers: { 'content-type': 'text/event-stream' } },
        )
    }
    const client = new StreamableHttpMcpClient(endpoint, '', fetcher)

    assert.deepEqual(await client.connect(), [])
})

test('rejects unsafe endpoints and duplicate or oversized tool catalogs', async () => {
    const unused: McpFetch = async () => {
        throw new Error('Must not request')
    }
    assert.throws(
        () => new StreamableHttpMcpClient('file:///etc/passwd', '', unused),
        /HTTP or HTTPS/,
    )
    assert.throws(
        () => new StreamableHttpMcpClient('https://user:pass@mcp.example.test', '', unused),
        /token field/,
    )
    assert.throws(
        () => new StreamableHttpMcpClient('https://mcp.example.test/mcp?token=secret', '', unused),
        /query parameters/,
    )
    const badCatalog: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as Record<string, unknown>
        if (body.method === 'initialize') return jsonResponse(1, { protocolVersion: '2025-03-26' })
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        return jsonResponse(2, { tools: [{ name: 'same' }, { name: 'same' }] })
    }
    await assert.rejects(
        new StreamableHttpMcpClient(endpoint, '', badCatalog).connect(),
        /duplicate tool name/,
    )
})

test('follows tools/list cursors and rejects a repeated cursor', async () => {
    const methods: string[] = []
    const fetcher: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as {
            id?: number
            method: string
            params?: Record<string, unknown>
        }
        methods.push(body.method)
        if (body.method === 'initialize')
            return jsonResponse(body.id!, { protocolVersion: '2025-03-26' })
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        if (body.params?.cursor === 'page-2')
            return jsonResponse(body.id!, { tools: [{ name: 'second' }] })
        return jsonResponse(body.id!, { tools: [{ name: 'first' }], nextCursor: 'page-2' })
    }
    const client = new StreamableHttpMcpClient(endpoint, '', fetcher)
    assert.deepEqual(
        (await client.connect()).map((tool) => tool.name),
        ['first', 'second'],
    )
    assert.deepEqual(methods, [
        'initialize',
        'notifications/initialized',
        'tools/list',
        'tools/list',
    ])

    const repeatedCursor: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as { id?: number; method: string }
        if (body.method === 'initialize')
            return jsonResponse(body.id!, { protocolVersion: '2025-03-26' })
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        return jsonResponse(body.id!, { tools: [], nextCursor: 'same' })
    }
    await assert.rejects(
        new StreamableHttpMcpClient(endpoint, '', repeatedCursor).connect(),
        /repeated a tools\/list cursor/,
    )
})

test('reports tool errors returned in a successful MCP response', async () => {
    const fetcher: McpFetch = async (_url, init) => {
        const body = JSON.parse(String(init.body)) as Record<string, unknown>
        if (body.method === 'initialize') return jsonResponse(1, { protocolVersion: '2025-03-26' })
        if (body.method === 'notifications/initialized') return new Response(null, { status: 202 })
        if (body.method === 'tools/list') return jsonResponse(2, { tools: [{ name: 'fails' }] })
        return jsonResponse(3, {
            isError: true,
            content: [{ type: 'text', text: 'tool rejected' }],
        })
    }
    const client = new StreamableHttpMcpClient(endpoint, '', fetcher)
    await client.connect()

    await assert.rejects(client.callTool('fails', {}), /tool rejected/)
})
