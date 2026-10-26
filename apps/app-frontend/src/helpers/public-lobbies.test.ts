import assert from 'node:assert/strict'
import { afterEach, test } from 'node:test'

import {
    buildPublicLobbyJoinRequest,
    fetchPublicLobbies,
    type PublicLobby,
} from './public-lobbies.ts'

type SocketMessageHandler = (event: MessageEvent<string>) => void

class MockWebSocket {
    static instances: MockWebSocket[] = []

    onopen: ((event: Event) => void) | null = null
    onmessage: SocketMessageHandler | null = null
    onerror: ((event: Event) => void) | null = null
    onclose: ((event: CloseEvent) => void) | null = null
    readonly url: string
    readonly sent: string[] = []
    closed = false

    constructor(url: string) {
        this.url = url
        MockWebSocket.instances.push(this)
    }

    send(value: string) {
        this.sent.push(value)
    }

    close() {
        this.closed = true
    }

    open() {
        this.onopen?.(new Event('open'))
    }

    message(value: string) {
        this.onmessage?.({ data: value } as MessageEvent<string>)
    }

    respond(value: unknown) {
        this.message(JSON.stringify(value))
    }
}

const originalWebSocket = globalThis.WebSocket

function installMockWebSocket() {
    MockWebSocket.instances = []
    Object.defineProperty(globalThis, 'WebSocket', {
        configurable: true,
        value: MockWebSocket as unknown as typeof WebSocket,
    })
}

afterEach(() => {
    Object.defineProperty(globalThis, 'WebSocket', {
        configurable: true,
        value: originalWebSocket,
    })
})

test('requests the public listing and returns sanitized directory fields', async () => {
    installMockWebSocket()
    const pending = fetchPublicLobbies(100)
    const socket = MockWebSocket.instances[0]

    assert.equal(socket.url, 'wss://mctier.pmhs.top/signaling')
    socket.open()
    assert.deepEqual(JSON.parse(socket.sent[0]), { type: 'public-lobby-list-request' })
    socket.respond({
        type: 'public-lobby-list-response',
        lobbies: [
            {
                lobbyName: '  \u0000Builders  ',
                hostName: ' Alice ',
                playerCount: 2.8,
                maxPlayers: 8,
                description: ' Friendly room ',
                serverNode: 'udp://untrusted.example:11010',
                password: 'never expose this',
            },
            { lobbyName: '', hostName: 'Missing name' },
        ],
    })

    assert.deepEqual(await pending, [
        {
            lobbyName: 'Builders',
            hostName: 'Alice',
            playerCount: 2,
            maxPlayers: 8,
            description: 'Friendly room',
            serverNode: 'udp://untrusted.example:11010',
        },
    ])
    assert.equal(socket.closed, true)
})

test('ignores unrelated frames and resolves an empty listing', async () => {
    installMockWebSocket()
    const pending = fetchPublicLobbies(100)
    const socket = MockWebSocket.instances[0]
    socket.open()
    socket.message('{invalid json')
    socket.respond({ type: 'heartbeat' })
    socket.respond({ type: 'public-lobby-list-response', lobbies: [] })

    assert.deepEqual(await pending, [])
})

test('rejects malformed responses, connection errors, and timeouts', async () => {
    installMockWebSocket()
    const malformed = fetchPublicLobbies(100)
    const malformedSocket = MockWebSocket.instances[0]
    malformedSocket.open()
    malformedSocket.respond({ type: 'public-lobby-list-response', lobbies: null })
    await assert.rejects(malformed, /response was invalid/i)

    installMockWebSocket()
    const failed = fetchPublicLobbies(100)
    MockWebSocket.instances[0].onerror?.(new Event('error'))
    await assert.rejects(failed, /could not load/i)

    installMockWebSocket()
    await assert.rejects(fetchPublicLobbies(1), /timed out/i)
    assert.equal(MockWebSocket.instances[0].closed, true)
})

test('closes the request when the page cancels it', async () => {
    installMockWebSocket()
    const controller = new AbortController()
    const pending = fetchPublicLobbies(100, controller.signal)
    controller.abort()

    await assert.rejects(pending, { name: 'AbortError' })
    assert.equal(MockWebSocket.instances[0].closed, true)
})

test('bounds listing size and numeric counts from the remote service', async () => {
    installMockWebSocket()
    const pending = fetchPublicLobbies(100)
    MockWebSocket.instances[0].respond({
        type: 'public-lobby-list-response',
        lobbies: Array.from({ length: 350 }, () => ({
            lobbyName: 'A'.repeat(100),
            hostName: 'Host',
            playerCount: -1,
            maxPlayers: 200_000,
            desc: 'D'.repeat(300),
        })),
    })

    const result = await pending
    assert.equal(result.length, 300)
    assert.equal(result[0].lobbyName.length, 64)
    assert.equal(result[0].description.length, 200)
    assert.equal(result[0].playerCount, 0)
    assert.equal(result[0].maxPlayers, 100_000)
})

test('a closed connection and an oversized frame fail instead of waiting until timeout', async () => {
    installMockWebSocket()
    const closed = fetchPublicLobbies(100)
    MockWebSocket.instances[0].onclose?.({} as CloseEvent)
    await assert.rejects(closed, /could not load/i)

    installMockWebSocket()
    const oversized = fetchPublicLobbies(100)
    MockWebSocket.instances[0].message('x'.repeat(500_001))
    await assert.rejects(oversized, /too large/i)
})

test('builds a real MCTier join request without trusting missing port or secret fields', () => {
    const lobby: PublicLobby = {
        lobbyName: 'Builders',
        hostName: 'Alice',
        playerCount: 2,
        maxPlayers: 8,
        description: '',
        serverNode: 'tcp://node.example:11010',
    }

    assert.deepEqual(buildPublicLobbyJoinRequest(lobby, 'Alex', 25565), {
        mode: 'join',
        lobby_name: 'Builders',
        password: '',
        player_name: 'Alex',
        server_node: 'tcp://node.example:11010',
        signaling_server: 'wss://mctier.pmhs.top/signaling',
        game_port: 25565,
    })
    assert.throws(() => buildPublicLobbyJoinRequest(lobby, 'Alex', 0), /port/i)
    assert.throws(() => buildPublicLobbyJoinRequest(lobby, 'Alex', 65536), /port/i)
})
