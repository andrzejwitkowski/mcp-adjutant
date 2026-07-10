import { LLM_CLIENTS } from './clients'
import type { PhaseProfile, Provider } from './types'
import { emitUiNotify } from './uiLog'

interface LlmClientCatalogProps {
  groupName: string
  profile: PhaseProfile
  onChange: (profile: PhaseProfile) => void
}

function clientDefaults(provider: Provider): Pick<PhaseProfile, 'base_url' | 'model_name'> {
  const client = LLM_CLIENTS.find((entry) => entry.provider === provider)
  return {
    base_url: client?.defaultBaseUrl ?? '',
    model_name: client?.defaultModel ?? '',
  }
}

export function LlmClientCatalog({
  groupName,
  profile,
  onChange,
}: LlmClientCatalogProps) {
  return (
    <div className="llm-client-catalog">
      {LLM_CLIENTS.map((client) => {
        const selected = profile.provider === client.provider

        return (
          <section
            key={client.provider}
            className={`llm-client-card${selected ? ' is-selected' : ''}`}
          >
            <header className="llm-client-card__header">
              <label className="llm-client-card__select">
                <input
                  type="radio"
                  name={`provider-${groupName}`}
                  checked={selected}
                  onChange={() => {
                    emitUiNotify({
                      subject: {
                        component: 'llm-catalog',
                        summary: `provider selected: ${client.provider}`,
                      },
                      meta: {
                          sourceModule: 'config-ui/LlmClientCatalog',
                        correlationId: null,
                      },
                    })
                    onChange({
                      ...profile,
                      provider: client.provider,
                      ...clientDefaults(client.provider),
                    })
                  }}
                />
                <span className="llm-client-card__title">{client.label}</span>
              </label>
              <p className="llm-client-card__description">{client.description}</p>
            </header>

            {selected && (
              <div className="llm-client-card__fields">
                <label>
                  API key
                  <input
                    type="password"
                    value={profile.api_key ?? ''}
                    placeholder="sk-..."
                    onChange={(event) =>
                      onChange({
                        ...profile,
                        api_key: event.target.value || null,
                      })
                    }
                  />
                </label>
                <label>
                  Base URL
                  <input
                    type="url"
                    value={profile.base_url}
                    onChange={(event) =>
                      onChange({ ...profile, base_url: event.target.value })
                    }
                  />
                </label>
                <label>
                  Model
                  <input
                    type="text"
                    value={profile.model_name}
                    onChange={(event) =>
                      onChange({ ...profile, model_name: event.target.value })
                    }
                  />
                </label>
                <label>
                  Max tokens
                  <input
                    type="number"
                    min={256}
                    step={256}
                    value={profile.max_tokens}
                    onChange={(event) =>
                      onChange({
                        ...profile,
                        max_tokens: Number(event.target.value),
                      })
                    }
                  />
                </label>
                <label>
                  Temperature
                  <input
                    type="number"
                    min={0}
                    max={2}
                    step={0.1}
                    value={profile.temperature}
                    onChange={(event) =>
                      onChange({
                        ...profile,
                        temperature: Number(event.target.value),
                      })
                    }
                  />
                </label>
              </div>
            )}
          </section>
        )
      })}
    </div>
  )
}
