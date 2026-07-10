export type Provider = 'deep_seek' | 'open_router' | 'open_ai' | 'custom'

export type AgentPhase = 'scout' | 'triage' | 'builder' | 'transformer' | 'evaluator'

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

export interface AgentEvaluationRow {
  id: string
  agent_name: string
  original_task: string
  agent_output: string
  score: number
  feedback_notes: string
  created_at: number
}

export interface EvaluationsPage {
  items: AgentEvaluationRow[]
  page: number
  page_size: number
  total_count: number
  total_pages: number
  avg_score: number | null
}

export interface CacheOverview {
  project_root: string
  query_count: number
  insight_count: number
  code_node_count: number
  embedding_count: number
  dependency_count: number
  evaluation_count: number
}

export interface CachedQueryRow {
  id: string
  raw_text: string
  has_embedding: boolean
}

export interface CachedInsightRow {
  id: string
  query_text: string | null
  content: string
  created_at: number
}

export interface CodeNodeRow {
  id: string
  file_path: string
  last_known_git_sha: string | null
  last_known_mtime: number
  is_dirty: boolean
}

export interface InsightDependencyRow {
  insight_id: string
  code_node_id: string
}

export interface CacheSnapshot {
  overview: CacheOverview
  queries: CachedQueryRow[]
  insights: CachedInsightRow[]
  code_nodes: CodeNodeRow[]
  dependencies: InsightDependencyRow[]
}

export interface LlmClientDefinition {
  provider: Provider
  label: string
  description: string
  defaultBaseUrl: string
  defaultModel: string
}

export interface UiNotifyHeadline {
  component: string
  summary: string
}

export interface UiNotifyMeta {
  sourceModule: string
  correlationId?: string | null
}

export interface UiNotifyEvent {
  subject: UiNotifyHeadline
  meta: UiNotifyMeta
}
