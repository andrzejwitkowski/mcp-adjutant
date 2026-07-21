import type { AgentPhase } from './types'

export const AGENT_PHASES: {
  phase: AgentPhase
  title: string
  hint: string
  icon: string
}[] = [
  {
    phase: 'scout',
    title: 'Scout',
    hint: 'Discovery & codebase retrieval',
    icon: 'search',
  },
  {
    phase: 'triage',
    title: 'Triage',
    hint: 'Compiler errors & trivial fixes',
    icon: 'rule',
  },
  {
    phase: 'builder',
    title: 'Builder',
    hint: 'Test generation & scaffolding',
    icon: 'construction',
  },
  {
    phase: 'transformer',
    title: 'Transformer',
    hint: 'Global AST refactors',
    icon: 'auto_fix_high',
  },
  {
    phase: 'evaluator',
    title: 'Evaluator',
    hint: 'QA sub-agent output (1–10)',
    icon: 'grade',
  },
  {
    phase: 'log_analyzer',
    title: 'Log Analyzer',
    hint: 'Triage log files',
    icon: 'terminal',
  },
  {
    phase: 'web_fetcher',
    title: 'Web Fetcher',
    hint: 'Web doc research',
    icon: 'travel_explore',
  },
  {
    phase: 'babysitter',
    title: 'Babysitter',
    hint: 'PR orchestration',
    icon: 'support_agent',
  },
  {
    phase: 'planner',
    title: 'Planner (scout)',
    hint: 'Cheap scouting before emit',
    icon: 'map',
  },
  {
    phase: 'planner_emit',
    title: 'Planner (emit)',
    hint: 'Blueprint JSON synthesis',
    icon: 'edit_note',
  },
]
