import { useEffect, useState } from 'react'
import { LlmClientCatalog } from './LlmClientCatalog'
import { NavBar } from './NavBar'
import type { AdjutantConfig, AgentPhase, PhaseProfile, WebFetcherProfile } from './types'
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
  const phases = { ...loaded.phases }
  for (const { phase } of AGENT_PHASES) {
    if (!phases[phase]) {
      phases[phase] = { ...DEFAULT_PROFILE }
    }
  }
  return { ...loaded, phases }
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
    setConfig((current) => {
      const existing = current.web_fetcher ?? {
        browsing: { ...DEFAULT_PROFILE },
        max_search_hops: 3,
        token_budget: 8000,
      }
      return { ...current, web_fetcher: { ...existing, ...patch } }
    })
  }

  async function saveConfig() {
    if (!loaded) return

    setStatus('saving')
    setMessage('')
    try {
      const response = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(config),
      })
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const saved = (await response.json()) as AdjutantConfig
      setConfig(saved)
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
          <h2>Web Fetcher — browsing model</h2>
          <p>
            The browsing-capable model (OpenRouter :online / Perplexity Sonar) that
            performs live web searches inside the search_web tool.
          </p>
        </header>
        <LlmClientCatalog
          groupName="web_fetcher_browsing"
          profile={config.web_fetcher?.browsing ?? { ...DEFAULT_PROFILE }}
          onChange={(profile) => updateWebFetcher({ browsing: profile })}
        />
        <label className="config-app__tunable">
          Max search hops
          <input
            type="number"
            min={1}
            max={10}
            value={config.web_fetcher?.max_search_hops ?? 3}
            onChange={(e) =>
              updateWebFetcher({
                max_search_hops: Number(e.target.value),
              })
            }
          />
        </label>
        <label className="config-app__tunable">
          Token budget
          <input
            type="number"
            min={1000}
            step={1000}
            value={config.web_fetcher?.token_budget ?? 8000}
            onChange={(e) =>
              updateWebFetcher({
                token_budget: Number(e.target.value),
              })
            }
          />
        </label>
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
