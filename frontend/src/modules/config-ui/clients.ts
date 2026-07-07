import type { LlmClientDefinition } from './types'

// ponytail: one list, all OpenAI-compatible transports the backend factory supports
export const LLM_CLIENTS: LlmClientDefinition[] = [
  {
    provider: 'deep_seek',
    label: 'DeepSeek',
    description: 'DeepSeek API (OpenAI-compatible)',
    defaultBaseUrl: 'https://api.deepseek.com/v1',
    defaultModel: 'deepseek-chat',
  },
  {
    provider: 'open_router',
    label: 'OpenRouter',
    description: 'OpenRouter gateway (multi-model)',
    defaultBaseUrl: 'https://openrouter.ai/api/v1',
    defaultModel: 'deepseek/deepseek-chat',
  },
  {
    provider: 'open_ai',
    label: 'OpenAI',
    description: 'OpenAI API',
    defaultBaseUrl: 'https://api.openai.com/v1',
    defaultModel: 'gpt-4o-mini',
  },
  {
    provider: 'custom',
    label: 'Custom',
    description: 'Any OpenAI-compatible endpoint',
    defaultBaseUrl: '',
    defaultModel: '',
  },
]
