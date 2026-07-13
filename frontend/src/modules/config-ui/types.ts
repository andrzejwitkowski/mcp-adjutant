export type Provider = 'deep_seek' | 'open_router' | 'open_ai' | 'custom'

export type AgentPhase =
  | 'scout'
  | 'triage'
  | 'builder'
  | 'transformer'
  | 'evaluator'
  | 'log_analyzer'
  | 'web_fetcher'
  | 'babysitter'

export interface PhaseProfile {
  provider: Provider
  api_key: string | null
  base_url: string
  model_name: string
  max_tokens: number
  temperature: number
}

export interface WebFetcherProfile {
  brave_api_key?: string | null
  max_search_hops: number
  token_budget: number
  cache_ttl_seconds: number
  web_cache_threshold: number
}

export interface AdjutantConfig {
  phases: Partial<Record<AgentPhase, PhaseProfile>>
  server_port: number
  storage_path: string
  triage_overrides?: Record<string, string> | null
  web_fetcher?: WebFetcherProfile | null
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
  web_query_count: number
  web_report_count: number
  web_source_count: number
  web_dependency_count: number
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

export interface WebQueryRow {
  id: string
  raw_text: string
  has_embedding: boolean
}

export interface WebReportRow {
  id: string
  query_text: string | null
  content: string
  created_at: number
}

export interface WebSourceRow {
  id: string
  url: string
  content_sha256: string
  fetched_at: number
  is_stale: boolean
}

export interface WebFetchDependencyRow {
  report_id: string
  source_id: string
}

export interface CacheSnapshot {
  overview: CacheOverview
  queries: CachedQueryRow[]
  insights: CachedInsightRow[]
  code_nodes: CodeNodeRow[]
  dependencies: InsightDependencyRow[]
  web_queries: WebQueryRow[]
  web_reports: WebReportRow[]
  web_sources: WebSourceRow[]
  web_dependencies: WebFetchDependencyRow[]
}

export interface ScoutCachePage {
  overview: CacheOverview
  queries: CachedQueryRow[]
  insights: CachedInsightRow[]
  code_nodes: CodeNodeRow[]
  dependencies: InsightDependencyRow[]
  page: number
  page_size: number
  total_count: number
  total_pages: number
}

export interface WebCachePage {
  overview: CacheOverview
  web_queries: WebQueryRow[]
  web_reports: WebReportRow[]
  web_sources: WebSourceRow[]
  web_dependencies: WebFetchDependencyRow[]
  page: number
  page_size: number
  total_count: number
  total_pages: number
}

export interface CacheHitSummary {
  scout: number
  web_fetcher: number
}

export interface PhaseTokenSummary {
  agent_phase: string
  prompt_tokens: number
  completion_tokens: number
  job_runs: number
}

export interface MetricsSummary {
  session_id: string
  utc_date: string
  prompt_tokens: number
  completion_tokens: number
  cache_hits: CacheHitSummary
  by_phase: PhaseTokenSummary[]
}

export interface DailyMetricsRow {
  date: string
  agent_phase: string
  prompt_tokens: number
  completion_tokens: number
  cache_hits: number
  job_runs: number
}

export interface TimelineBucket {
  hour: number
  agent_phase: string
  prompt_tokens: number
  completion_tokens: number
  cumulative_prompt_tokens: number
  cumulative_completion_tokens: number
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
