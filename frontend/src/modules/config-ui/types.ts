export type Provider = 'deep_seek' | 'open_router' | 'open_ai' | 'custom'

export type AgentPhase = 'scout' | 'triage' | 'builder'

export interface PhaseProfile {
  provider: Provider
  api_key: string | null
  base_url: string
  model_name: string
  max_tokens: number
  temperature: number
}

export interface AdjutantConfig {
  phases: Partial<Record<AgentPhase, PhaseProfile>>
  server_port: number
  storage_path: string
  triage_overrides?: Record<string, string> | null
}

export interface LlmClientDefinition {
  provider: Provider
  label: string
  description: string
  defaultBaseUrl: string
  defaultModel: string
}
