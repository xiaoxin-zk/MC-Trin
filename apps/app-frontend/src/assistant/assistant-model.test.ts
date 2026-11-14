import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    assistantModelKey,
    assistantModelOptions,
    findAssistantModel,
    groupAssistantModels,
    parseAssistantModelValue,
} from './assistant-model.ts'
import type { AIProviderConfig, AIProviderDefinition } from '../helpers/ai.ts'

function provider(
    providerId: string,
    models: { id: string; name?: string; enabled?: boolean }[],
    overrides: Partial<AIProviderConfig> = {},
): AIProviderConfig {
    return {
        provider_id: providerId,
        custom_name: null,
        protocol: 'openai',
        enabled: true,
        endpoint: '',
        settings: {},
        has_api_key: true,
        configured_credentials: [],
        oauth_connected: false,
        models: models.map((model) => ({
            id: model.id,
            name: model.name ?? '',
            enabled: model.enabled ?? true,
            source: 'custom',
        })),
        ...overrides,
    }
}

const catalog: AIProviderDefinition[] = [
    {
        id: 'openai',
        name: 'OpenAI',
        protocol: 'openai',
        auth_type: 'apiKey',
        default_endpoint: '',
        check_model: '',
        show_model_fetcher: true,
        required_settings: [],
        sponsored: false,
    },
    {
        id: 'ollama',
        name: 'Ollama',
        protocol: 'ollama',
        auth_type: 'none',
        default_endpoint: '',
        check_model: '',
        show_model_fetcher: true,
        required_settings: [],
        sponsored: false,
    },
]

test('a provider/model pair survives the round trip through one option value', () => {
    const value = assistantModelKey('deepseek', 'deepseek-reasoner')
    assert.deepEqual(parseAssistantModelValue(value), {
        providerId: 'deepseek',
        modelId: 'deepseek-reasoner',
    })
})

test('an option value without both halves is refused', () => {
    assert.equal(parseAssistantModelValue(''), null)
    assert.equal(parseAssistantModelValue('openai'), null)
    assert.equal(parseAssistantModelValue('\u001fgpt-4o-mini'), null)
    assert.equal(parseAssistantModelValue('openai\u001f'), null)
})

test('only enabled models of usable providers become options', () => {
    const options = assistantModelOptions(
        [
            provider('openai', [
                { id: 'gpt-4o-mini', name: 'GPT-4o mini' },
                { id: 'disabled-model', name: 'Hidden', enabled: false },
            ]),
            provider('ollama', [{ id: 'llama3' }]),
            provider('no-key', [{ id: 'model' }], {
                has_api_key: false,
                configured_credentials: [],
            }),
            provider('off', [{ id: 'model' }], { enabled: false }),
        ],
        catalog,
    )

    assert.deepEqual(
        options.map((option) => `${option.providerId}/${option.modelId}`),
        ['openai/gpt-4o-mini', 'ollama/llama3'],
    )
})

test('a model without a display name is shown by its id', () => {
    const options = assistantModelOptions(
        [provider('openai', [{ id: 'gpt-4o-mini' }, { id: 'named', name: 'Friendly' }])],
        catalog,
    )

    assert.deepEqual(
        options.map((option) => option.modelLabel),
        ['gpt-4o-mini', 'Friendly'],
    )
})

test('a custom provider name labels the group instead of the catalog name', () => {
    const options = assistantModelOptions(
        [provider('openai', [{ id: 'gpt-4o-mini' }], { custom_name: 'Work account' })],
        catalog,
    )

    assert.equal(options[0].providerLabel, 'Work account')
    assert.equal(groupAssistantModels(options)[0].label, 'Work account')
})

test('a stored pair that is no longer offered resolves to nothing', () => {
    const options = assistantModelOptions([provider('openai', [{ id: 'gpt-4o-mini' }])], catalog)

    assert.ok(findAssistantModel(options, { providerId: 'openai', modelId: 'gpt-4o-mini' }))
    assert.equal(findAssistantModel(options, { providerId: 'openai', modelId: 'retired' }), null)
    assert.equal(
        findAssistantModel(options, { providerId: 'removed', modelId: 'gpt-4o-mini' }),
        null,
    )
    assert.equal(findAssistantModel(options, null), null)
})
