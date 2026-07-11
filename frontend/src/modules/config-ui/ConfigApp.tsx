import { useEffect, useState } from 'react'
import { LlmClientCatalog } from './LlmClientCatalog'
import { NavBar } from './NavBar'
import { WebFetcherTunables } from './WebFetcherTunables'
import type { AdjutantConfig, AgentPhase, PhaseProfile, WebFetcherProfile } from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

const AGENT_PHASES: { phase: AgentPhase; title: string; hint: string }[] = [
  {
    phase: 'scout',
    title: 'Scout',
    hint: 'Codebase scouting and context gathering',
  },
  {
    phase: 'triage',
    title: 'Triage',
    hint: 'Compiler errors and trivial fixes',
  },
  {
    phase: 'builder',
    title: 'Builder',
    hint: 'Test generation and scaffolding',
  },
  {
    phase: 'transformer',
    title: 'Transformer',
    hint: 'Global AST refactors and signature propagation',
  },
  {
    phase: 'evaluator',
    title: 'Evaluator',
    hint: 'QA sub-agent output quality (scores 1–10)',
  },
  {
    phase: 'web_fetcher',
    title: 'Web Fetcher',
    hint: 'Reasoning model that drives web doc research',
  },
]

const DEFAULT_PROFILE: PhaseProfile = {
  provider: 'deep_seek',
  api_key: null,
  base_url: 'https://api.deepseek.com/v1',
  model_name: 'deepseek-chat',
  max_tokens: 4096,
  temperature: 0.2,
}

const DEFAULT_WEB_FETCHER: WebFetcherProfile = {
  brave_api_key: null,
  max_search_hops: 3,
  token_budget: 8000,
  cache_ttl_seconds: 604800,
  web_cache_threshold: 0.78,
}

const KNOWN_PHASES: AgentPhase[] = [
  'scout',
  'triage',
  'builder',
  'transformer',
  'evaluator',
  'web_fetcher',
]

function defaultWebFetcher(): WebFetcherProfile {
  return { ...DEFAULT_WEB_FETCHER }
}

function migrateLegacyPhases(
  raw: Record<string, PhaseProfile>,
): Record<string, PhaseProfile> {
  const phases = { ...raw }
  if (!phases.pruner && phases.transformer) {
    phases.pruner = phases.transformer
    delete phases.transformer
  }
  return phases
}

function sanitizeConfigForSave(config: AdjutantConfig): AdjutantConfig {
  const rawPhases = migrateLegacyPhases(config.phases as Record<string, PhaseProfile>)

  const phases: Record<string, PhaseProfile> = {}
  for (const phase of [...KNOWN_PHASES, 'pruner', 'babysitter']) {
    const profile = rawPhases[phase]
    if (profile) {
      phases[phase] = profile
    }
  }

  return {
    ...config,
    phases: phases as AdjutantConfig['phases'],
    web_fetcher: config.web_fetcher ?? defaultWebFetcher(),
  }
}

function emptyConfig(): AdjutantConfig {
  return {
    phases: {},
    server_port: 3000,
    storage_path: '',
    triage_overrides: null,
    web_fetcher: null,
  }
}

function withDisplayedPhases(loaded: AdjutantConfig): AdjutantConfig {
  const phases = migrateLegacyPhases(loaded.phases as Record<string, PhaseProfile>) as Partial<
    Record<AgentPhase, PhaseProfile>
  >
  for (const { phase } of AGENT_PHASES) {
    if (!phases[phase]) {
      phases[phase] = { ...DEFAULT_PROFILE }
    }
  }
  return {
    ...loaded,
    phases,
    web_fetcher: loaded.web_fetcher ?? defaultWebFetcher(),
  }
}

export function ConfigApp() {
  const [config, setConfig] = useState<AdjutantConfig>(emptyConfig)
  const [loaded, setLoaded] = useState(false)
  const [status, setStatus] = useState<'loading' | 'ready' | 'saving' | 'error'>(
    'loading',
  )
  const [message, setMessage] = useState('')

  useEffect(() => {
    fetch('/api/config')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<AdjutantConfig>
      })
      .then((loadedConfig) => {
        setConfig(withDisplayedPhases(loadedConfig))
        setLoaded(true)
        setStatus('ready')
      })
      .catch((error: Error) => {
        emitUiNotify({
          subject: {
            component: 'config-app',
            summary: `load failed: ${error.message}`,
          },
          meta: {
              sourceModule: 'config-ui/ConfigApp',
            correlationId: null,
          },
        })
        setLoaded(false)
        setStatus('error')
        setMessage(error.message)
      })
  }, [])

  function profileFor(phase: AgentPhase): PhaseProfile {
    return config.phases[phase] ?? { ...DEFAULT_PROFILE }
  }

  function updatePhase(phase: AgentPhase, profile: PhaseProfile) {
    setConfig((current) => ({
      ...current,
      phases: { ...current.phases, [phase]: profile },
    }))
  }

  function updateWebFetcher(patch: Partial<WebFetcherProfile>) {
    setConfig((current) => ({
      ...current,
      web_fetcher: { ...(current.web_fetcher ?? defaultWebFetcher()), ...patch },
    }))
  }

  async function saveConfig() {
    if (!loaded) return

    setStatus('saving')
    setMessage('')
    const payload = sanitizeConfigForSave(config)
    try {
      const response = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      })
      if (!response.ok) {
        const detail = await response.text()
        throw new Error(detail || `HTTP ${response.status}`)
      }
      const saved = (await response.json()) as AdjutantConfig
      setConfig(withDisplayedPhases(saved))
      emitUiNotify({
        subject: {
          component: 'config-app',
          summary: 'configuration saved',
        },
        meta: {
          sourceModule: 'config-ui/ConfigApp',
          correlationId: null,
        },
      })
      setStatus('ready')
      setMessage('Saved')
    } catch (error) {
      setStatus('error')
      setMessage(error instanceof Error ? error.message : 'Save failed')
    }
  }

  if (status === 'loading') {
    return <main className="config-app">Loading configuration…</main>
  }

  if (!loaded) {
    return (
      <main className="config-app">
        <p className="config-app__message is-error">
          Failed to load configuration: {message || 'unknown error'}
        </p>
      </main>
    )
  }

  return (
    <main className="config-app">
      <NavBar />
      <header className="config-app__header">
        <h1>mcp-adjutant LLM config</h1>
        <p>Choose an OpenAI-compatible client per agent phase.</p>
        <div className="config-app__quick-links">
          <a href="#/evaluations">Agent evaluations</a>
          <a href="#/cache">Scout semantic cache</a>
          <a href="#/web-cache">Web fetcher cache</a>
        </div>
      </header>

      {AGENT_PHASES.map(({ phase, title, hint }) => (
        <section key={phase} className="agent-panel">
          <header>
            <h2>{title}</h2>
            <p>{hint}</p>
          </header>
          <LlmClientCatalog
            groupName={phase}
            profile={profileFor(phase)}
            onChange={(profile) => updatePhase(phase, profile)}
          />
        </section>
      ))}

      <section className="agent-panel">
        <header>
          <h2>Web Fetcher — tunables</h2>
          <p>Brave Search credentials and cache behavior for the web research agent.</p>
        </header>
        <WebFetcherTunables
          profile={config.web_fetcher ?? defaultWebFetcher()}
          onChange={updateWebFetcher}
        />
      </section>

      <footer className="config-app__footer">
        <button type="button" onClick={saveConfig} disabled={status === 'saving'}>
          {status === 'saving' ? 'Saving…' : 'Save configuration'}
        </button>
        {message && <p className={`config-app__message is-${status}`}>{message}</p>}
      </footer>
    </main>
  )
}
