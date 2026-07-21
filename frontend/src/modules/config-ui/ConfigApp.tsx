import { useEffect, useState } from 'react'
import { AGENT_PHASES } from './agents'
import { AgentPhaseCard } from './AgentPhaseCard'
import { AppShell } from './AppShell'
import { LLM_CLIENTS } from './clients'
import { ProviderProfilesPanel } from './ProviderProfilesPanel'
import type {
  AdjutantConfig,
  AgentPhase,
  PhaseBinding,
  Provider,
  ProviderProfile,
  WebFetcherProfile,
} from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

const DEFAULT_PROFILE_ID = 'default'

const DEFAULT_BINDING: PhaseBinding = {
  profile_id: DEFAULT_PROFILE_ID,
  model_name: 'deepseek-chat',
  max_tokens: 4096,
  temperature: 0.2,
}

const PHASE_BINDING_OVERRIDES: Partial<Record<AgentPhase, Partial<PhaseBinding>>> = {
  evaluator: { max_tokens: 2048, temperature: 0 },
  log_analyzer: { max_tokens: 2048, temperature: 0 },
  web_fetcher: { max_tokens: 2048, temperature: 0.2 },
  babysitter: { max_tokens: 4096, temperature: 0.4 },
  builder: { model_name: 'deepseek-coder', max_tokens: 8192, temperature: 0.2 },
  planner: { max_tokens: 4096, temperature: 0.3 },
  planner_emit: { model_name: 'deepseek-coder', max_tokens: 8192, temperature: 0.1 },
  transformer: { model_name: 'deepseek-coder', max_tokens: 8192, temperature: 0.1 },
  triage: { model_name: 'deepseek-coder', max_tokens: 4096, temperature: 0 },
  git_janitor: { max_tokens: 4096, temperature: 0.2 },
}

const DEFAULT_WEB_FETCHER: WebFetcherProfile = {
  brave_api_key: null,
  max_search_hops: 3,
  token_budget: 8000,
  cache_ttl_seconds: 604800,
  web_cache_threshold: 0.78,
}

/** Flat per-phase shape from pre-profile config / old binaries. */
type LegacyPhase = PhaseBinding & {
  provider?: Provider
  api_key?: string | null
  base_url?: string
}

function defaultBinding(phase: AgentPhase): PhaseBinding {
  return { ...DEFAULT_BINDING, ...PHASE_BINDING_OVERRIDES[phase] }
}

function defaultProvider(): ProviderProfile {
  return {
    id: DEFAULT_PROFILE_ID,
    name: 'DeepSeek Default',
    provider: 'deep_seek',
    api_key: null,
    base_url: 'https://api.deepseek.com/v1',
  }
}

function providerLabel(provider: string): string {
  return LLM_CLIENTS.find((c) => c.provider === provider)?.label ?? provider
}

function emptyConfig(): AdjutantConfig {
  return {
    profiles: {},
    default_profile_id: null,
    phases: {},
    server_port: 3000,
    storage_path: '',
    triage_overrides: null,
    web_fetcher: null,
  }
}

/** Collapse legacy flat phases (provider+key on each agent) into shared profiles. */
function migrateLegacyFlatPhases(loaded: AdjutantConfig): AdjutantConfig {
  const rawPhases = loaded.phases as Partial<Record<AgentPhase, LegacyPhase>>
  const hasProfiles = Object.keys(loaded.profiles ?? {}).length > 0
  const isLegacy = Object.values(rawPhases).some((p) => p?.provider != null)
  if (hasProfiles || !isLegacy) return loaded

  const profiles: Record<string, ProviderProfile> = {}
  const dedupe = new Map<string, string>()
  const phases: AdjutantConfig['phases'] = {}
  const counts = new Map<string, number>()
  let next = 0

  for (const [phase, obj] of Object.entries(rawPhases) as [AgentPhase, LegacyPhase][]) {
    if (!obj) continue
    const provider = obj.provider ?? 'deep_seek'
    const base_url = obj.base_url ?? 'https://api.deepseek.com/v1'
    const api_key = obj.api_key ?? null
    const key = `${provider}|${base_url}|${api_key ?? ''}`
    let profileId = dedupe.get(key)
    if (!profileId) {
      next += 1
      profileId = next === 1 ? DEFAULT_PROFILE_ID : `profile-${next}`
      dedupe.set(key, profileId)
      profiles[profileId] = {
        id: profileId,
        name:
          next === 1
            ? `${providerLabel(provider)} Default`
            : `${providerLabel(provider)} ${next}`,
        provider,
        api_key,
        base_url,
      }
    }
    counts.set(profileId, (counts.get(profileId) ?? 0) + 1)
    phases[phase] = {
      profile_id: profileId,
      model_name: obj.model_name,
      max_tokens: obj.max_tokens,
      temperature: obj.temperature,
    }
  }

  const defaultId =
    [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? DEFAULT_PROFILE_ID

  return {
    ...loaded,
    profiles,
    default_profile_id: defaultId,
    phases,
  }
}

function withDisplayed(loaded: AdjutantConfig): AdjutantConfig {
  const migrated = migrateLegacyFlatPhases(loaded)
  const profiles = { ...migrated.profiles }
  if (Object.keys(profiles).length === 0) {
    profiles[DEFAULT_PROFILE_ID] = defaultProvider()
  }
  const defaultId =
    migrated.default_profile_id && profiles[migrated.default_profile_id]
      ? migrated.default_profile_id
      : Object.keys(profiles)[0]
  const phases = { ...migrated.phases }
  for (const { phase } of AGENT_PHASES) {
    if (!phases[phase]) {
      phases[phase] = { ...defaultBinding(phase), profile_id: defaultId }
    }
  }
  return {
    ...migrated,
    profiles,
    default_profile_id: defaultId,
    phases,
    web_fetcher: migrated.web_fetcher ?? { ...DEFAULT_WEB_FETCHER },
  }
}

function sanitize(config: AdjutantConfig): AdjutantConfig {
  const phases: AdjutantConfig['phases'] = {}
  for (const { phase } of AGENT_PHASES) {
    const b = config.phases[phase]
    if (b) phases[phase] = b
  }
  return {
    ...config,
    phases,
    web_fetcher: config.web_fetcher ?? { ...DEFAULT_WEB_FETCHER },
  }
}

export function ConfigApp() {
  const [config, setConfig] = useState<AdjutantConfig>(emptyConfig)
  const [loaded, setLoaded] = useState(false)
  const [status, setStatus] = useState<'loading' | 'ready' | 'saving' | 'error'>(
    'loading',
  )
  const [message, setMessage] = useState('')
  const [selectedProfileId, setSelectedProfileId] = useState<string | null>(null)
  const [baseline, setBaseline] = useState<AdjutantConfig | null>(null)

  useEffect(() => {
    fetch('/api/config')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<AdjutantConfig>
      })
      .then((loadedConfig) => {
        const next = withDisplayed(loadedConfig)
        setConfig(next)
        setBaseline(next)
        setSelectedProfileId(next.default_profile_id)
        setLoaded(true)
        setStatus('ready')
      })
      .catch((error: Error) => {
        emitUiNotify({
          subject: { component: 'config-app', summary: `load failed: ${error.message}` },
          meta: { sourceModule: 'config-ui/ConfigApp', correlationId: null },
        })
        setLoaded(false)
        setStatus('error')
        setMessage(error.message)
      })
  }, [])

  function updateBinding(phase: AgentPhase, binding: PhaseBinding) {
    setConfig((c) => ({ ...c, phases: { ...c.phases, [phase]: binding } }))
  }

  function updateProfile(profile: ProviderProfile) {
    setConfig((c) => ({
      ...c,
      profiles: { ...c.profiles, [profile.id]: profile },
    }))
  }

  function addProfile() {
    const id = `profile-${crypto.randomUUID().slice(0, 8)}`
    const profile: ProviderProfile = {
      id,
      name: 'New Profile',
      provider: 'open_router',
      api_key: null,
      base_url: 'https://openrouter.ai/api/v1',
    }
    setConfig((c) => ({ ...c, profiles: { ...c.profiles, [id]: profile } }))
    setSelectedProfileId(id)
  }

  function deleteProfile(id: string) {
    const inUse = Object.values(config.phases).some((b) => b?.profile_id === id)
    if (inUse || config.default_profile_id === id) {
      setMessage('Reassign agents (and default) before deleting this profile')
      setStatus('error')
      return
    }
    setConfig((c) => {
      const profiles = { ...c.profiles }
      delete profiles[id]
      return { ...c, profiles }
    })
    setSelectedProfileId(config.default_profile_id)
  }

  async function saveConfig() {
    if (!loaded) return
    setStatus('saving')
    setMessage('')
    try {
      const response = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(sanitize(config)),
      })
      if (!response.ok) {
        throw new Error((await response.text()) || `HTTP ${response.status}`)
      }
      const saved = withDisplayed((await response.json()) as AdjutantConfig)
      setConfig(saved)
      setBaseline(saved)
      emitUiNotify({
        subject: { component: 'config-app', summary: 'configuration saved' },
        meta: { sourceModule: 'config-ui/ConfigApp', correlationId: null },
      })
      setStatus('ready')
      setMessage('Saved')
    } catch (error) {
      setStatus('error')
      setMessage(error instanceof Error ? error.message : 'Save failed')
    }
  }

  function resetConfig() {
    if (!baseline) return
    setConfig(baseline)
    setSelectedProfileId(baseline.default_profile_id)
    setMessage('Reset')
    setStatus('ready')
  }

  if (status === 'loading') {
    return (
      <AppShell>
        <main className="config-canvas">Loading configuration…</main>
      </AppShell>
    )
  }

  if (!loaded) {
    return (
      <AppShell>
        <main className="config-canvas">
          <p className="config-app__message is-error">
            Failed to load configuration: {message || 'unknown error'}
          </p>
        </main>
      </AppShell>
    )
  }

  return (
    <AppShell>
      <header className="config-topbar">
        <div>
          <span className="config-topbar__brand">mcp-adjutant</span>
          <span className="config-topbar__sep">/</span>
          <span className="config-topbar__page">LLM Config</span>
        </div>
        <div className="config-topbar__actions">
          <button type="button" className="config-topbar__reset" onClick={resetConfig}>
            Reset
          </button>
          <button
            type="button"
            className="config-topbar__save"
            onClick={saveConfig}
            disabled={status === 'saving'}
          >
            {status === 'saving' ? 'Saving…' : 'Save'}
          </button>
        </div>
      </header>
      <div className="config-canvas">
        <ProviderProfilesPanel
          profiles={config.profiles}
          defaultProfileId={config.default_profile_id}
          selectedId={selectedProfileId}
          onSelect={setSelectedProfileId}
          onChange={updateProfile}
          onAdd={addProfile}
          onDelete={deleteProfile}
          onSetDefault={(id) =>
            setConfig((c) => ({ ...c, default_profile_id: id }))
          }
        />
        <section className="phases-section">
          <h2>Agent Phase Settings</h2>
          <div className="phases-grid">
            {AGENT_PHASES.map(({ phase, title, hint, icon }) => (
              <AgentPhaseCard
                key={phase}
                id={`agent-${phase}`}
                title={title}
                hint={hint}
                icon={icon}
                binding={config.phases[phase] ?? defaultBinding(phase)}
                profiles={config.profiles}
                onChange={(b) => updateBinding(phase, b)}
                webFetcher={
                  phase === 'web_fetcher'
                    ? (config.web_fetcher ?? DEFAULT_WEB_FETCHER)
                    : undefined
                }
                onWebFetcherChange={
                  phase === 'web_fetcher'
                    ? (patch) =>
                        setConfig((c) => ({
                          ...c,
                          web_fetcher: {
                            ...(c.web_fetcher ?? DEFAULT_WEB_FETCHER),
                            ...patch,
                          },
                        }))
                    : undefined
                }
              />
            ))}
          </div>
        </section>
        {message && (
          <p className={`config-app__message is-${status === 'error' ? 'error' : 'ready'}`}>
            {message}
          </p>
        )}
      </div>
    </AppShell>
  )
}
