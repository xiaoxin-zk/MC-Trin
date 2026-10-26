import {
    BaseChatModel,
    type BaseChatModelCallOptions,
    type BindToolsInput,
} from '@langchain/core/language_models/chat_models'
import { AIMessage, type BaseMessage, ToolMessage } from '@langchain/core/messages'
import type { ChatResult } from '@langchain/core/outputs'
import { convertToOpenAITool } from '@langchain/core/utils/function_calling'

import type { AgentGateway, ModelMessage, ModelToolDefinition } from './types.ts'

interface ModelOptions extends BaseChatModelCallOptions {
    tools?: ModelToolDefinition[]
}

export function readableMessageContent(value: unknown): string {
    if (typeof value === 'string') return value
    if (Array.isArray(value)) {
        return value
            .filter(
                (block) =>
                    !block ||
                    typeof block !== 'object' ||
                    !('type' in block) ||
                    (block as { type?: unknown }).type === 'text',
            )
            .map(readableMessageContent)
            .filter(Boolean)
            .join('\n')
    }
    if (value && typeof value === 'object') {
        const record = value as Record<string, unknown>
        if (typeof record.type === 'string' && record.type !== 'text') return ''
        if (record.text !== undefined) return readableMessageContent(record.text)
        if (typeof record.content === 'string') return record.content
        if (Array.isArray(record.content)) return readableMessageContent(record.content)
        try {
            return JSON.stringify(value) ?? ''
        } catch {
            return ''
        }
    }
    return value == null ? '' : String(value)
}

function serializeMessage(message: BaseMessage): ModelMessage {
    const type = message.type
    return {
        role: type === 'human' ? 'user' : type === 'ai' ? 'assistant' : type,
        content: readableMessageContent(message.content),
        tool_calls: AIMessage.isInstance(message)
            ? message.tool_calls?.map((call) => ({
                  id: call.id ?? '',
                  name: call.name,
                  args: call.args,
              }))
            : undefined,
        tool_call_id: ToolMessage.isInstance(message) ? message.tool_call_id : undefined,
        opaque: message.additional_kwargs.logagent_opaque,
    }
}

/** Keeps credentials and transport in Rust while preserving opaque provider state. */
export class LogAgentModel extends BaseChatModel<ModelOptions> {
    modelName = 'axolotl-logagent:diagnostics'
    private gateway: AgentGateway

    constructor(gateway: AgentGateway) {
        super({ callbacks: [], disableStreaming: true })
        this.gateway = gateway
    }

    _llmType() {
        return 'axolotl-logagent'
    }

    get callKeys() {
        return [...super.callKeys, 'tools']
    }

    bindTools(tools: BindToolsInput[]) {
        return this.withConfig({ tools: tools.map((tool) => convertToOpenAITool(tool)) })
    }

    async _generate(
        messages: BaseMessage[],
        options: this['ParsedCallOptions'],
    ): Promise<ChatResult> {
        options.signal?.throwIfAborted()
        const result = await this.gateway.modelTurn({
            messages: messages.map(serializeMessage),
            tools: options.tools ?? [],
        })
        options.signal?.throwIfAborted()
        const message = new AIMessage({
            content: result.content,
            tool_calls: result.tool_calls?.map((call) => ({ ...call, type: 'tool_call' as const })),
            additional_kwargs: { logagent_opaque: result.opaque },
        })
        return { generations: [{ text: result.content, message }] }
    }
}
