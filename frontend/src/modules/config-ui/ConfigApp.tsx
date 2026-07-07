import { useEffect, useState } from 'react'
import { LlmClientCatalog } from './LlmClientCatalog'
import type { AdjutantConfig, AgentPhase, PhaseProfile } from './types'
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
  }
}

export function ConfigApp() {
  const [config, setConfig] = useState<AdjutantConfig>(emptyConfig)
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
      .then((loaded) => {
        setConfig(loaded)
        setStatus('ready')
      })
      .catch((error: Error) => {
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

  async function saveConfig() {
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

  return (
    <main className="config-app">
      <header className="config-app__header">
        <h1>mcp-adjutant LLM config</h1>
        <p>Choose an OpenAI-compatible client per agent phase.</p>
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

      <footer className="config-app__footer">
        <button type="button" onClick={saveConfig} disabled={status === 'saving'}>
          {status === 'saving' ? 'Saving…' : 'Save configuration'}
        </button>
        {message && <p className={`config-app__message is-${status}`}>{message}</p>}
      </footer>
    </main>
  )
}
