import type { WebFetcherProfile } from './types'

interface WebFetcherTunablesProps {
  profile: WebFetcherProfile
  onChange: (patch: Partial<WebFetcherProfile>) => void
}

function parseUint(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10)
  return Number.isFinite(parsed) ? parsed : fallback
}

function parseFloatValue(value: string, fallback: number): number {
  const parsed = Number.parseFloat(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

export function WebFetcherTunables({ profile, onChange }: WebFetcherTunablesProps) {
  return (
    <div className="tunable-card">
      <section className="tunable-card__section">
        <h3>Brave Search</h3>
        <p className="tunable-card__hint">
          Required for live web research. Get a key from{' '}
          <a href="https://api.search.brave.com/" target="_blank" rel="noreferrer">
            Brave Search API
          </a>
          .
        </p>
        <label>
          API key
          <input
            type="password"
            value={profile.brave_api_key ?? ''}
            placeholder="BSA..."
            autoComplete="off"
            onChange={(event) =>
              onChange({ brave_api_key: event.target.value || null })
            }
          />
        </label>
      </section>

      <section className="tunable-card__section">
        <h3>Agent limits</h3>
        <div className="tunable-card__grid">
          <label>
            Max search hops
            <input
              type="number"
              min={1}
              max={10}
              value={profile.max_search_hops}
              onChange={(event) =>
                onChange({
                  max_search_hops: parseUint(event.target.value, profile.max_search_hops),
                })
              }
            />
          </label>
          <label>
            Token budget
            <input
              type="number"
              min={1000}
              step={1000}
              value={profile.token_budget}
              onChange={(event) =>
                onChange({
                  token_budget: parseUint(event.target.value, profile.token_budget),
                })
              }
            />
          </label>
        </div>
      </section>

      <section className="tunable-card__section">
        <h3>Semantic cache</h3>
        <div className="tunable-card__grid">
          <label>
            Cache TTL (seconds)
            <input
              type="number"
              min={3600}
              step={3600}
              value={profile.cache_ttl_seconds}
              onChange={(event) =>
                onChange({
                  cache_ttl_seconds: parseUint(
                    event.target.value,
                    profile.cache_ttl_seconds,
                  ),
                })
              }
            />
          </label>
          <label>
            Similarity threshold
            <input
              type="number"
              min={0.5}
              max={1}
              step={0.01}
              value={profile.web_cache_threshold}
              onChange={(event) =>
                onChange({
                  web_cache_threshold: parseFloatValue(
                    event.target.value,
                    profile.web_cache_threshold,
                  ),
                })
              }
            />
          </label>
        </div>
      </section>
    </div>
  )
}
