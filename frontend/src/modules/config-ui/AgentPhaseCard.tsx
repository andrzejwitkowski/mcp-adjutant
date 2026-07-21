import type { PhaseBinding, ProviderProfile, WebFetcherProfile } from './types'
import { WebFetcherTunables } from './WebFetcherTunables'

interface Props {
  id: string
  title: string
  hint: string
  icon: string
  binding: PhaseBinding
  profiles: Record<string, ProviderProfile>
  onChange: (binding: PhaseBinding) => void
  webFetcher?: WebFetcherProfile
  onWebFetcherChange?: (patch: Partial<WebFetcherProfile>) => void
}

function parseUint(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10)
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback
}

function parseFloatValue(value: string, fallback: number): number {
  const parsed = Number.parseFloat(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

export function AgentPhaseCard({
  id,
  title,
  hint,
  icon,
  binding,
  profiles,
  onChange,
  webFetcher,
  onWebFetcherChange,
}: Props) {
  return (
    <section id={id} className="phase-card">
      <header className="phase-card__header">
        <div className="phase-card__title-row">
          <div className="phase-card__icon">
            <span className="material-symbols-outlined">{icon}</span>
          </div>
          <div>
            <h3>{title}</h3>
            <p>{hint}</p>
          </div>
        </div>
      </header>
      <div className="phase-card__grid">
        <label>
          Profile
          <select
            value={binding.profile_id}
            onChange={(e) => onChange({ ...binding, profile_id: e.target.value })}
          >
            {Object.values(profiles).map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Model
          <input
            type="text"
            value={binding.model_name}
            onChange={(e) => onChange({ ...binding, model_name: e.target.value })}
          />
        </label>
        <label>
          <span className="phase-card__temp-label">
            Temp <span>{binding.temperature.toFixed(1)}</span>
          </span>
          <input
            type="range"
            min={0}
            max={2}
            step={0.1}
            value={binding.temperature}
            onChange={(e) =>
              onChange({
                ...binding,
                temperature: parseFloatValue(e.target.value, binding.temperature),
              })
            }
          />
        </label>
        <label>
          Max Tokens
          <input
            type="number"
            min={256}
            step={256}
            value={binding.max_tokens}
            onChange={(e) =>
              onChange({
                ...binding,
                max_tokens: Math.max(256, parseUint(e.target.value, binding.max_tokens)),
              })
            }
          />
        </label>
      </div>
      {webFetcher && onWebFetcherChange && (
        <details className="phase-card__advanced">
          <summary>Web Fetcher tunables</summary>
          <WebFetcherTunables profile={webFetcher} onChange={onWebFetcherChange} />
        </details>
      )}
    </section>
  )
}
