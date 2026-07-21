import { LLM_CLIENTS } from './clients'
import type { Provider, ProviderProfile } from './types'

interface Props {
  profiles: Record<string, ProviderProfile>
  defaultProfileId: string | null
  selectedId: string | null
  onSelect: (id: string) => void
  onChange: (profile: ProviderProfile) => void
  onAdd: () => void
  onDelete: (id: string) => void
  onSetDefault: (id: string) => void
}

export function ProviderProfilesPanel({
  profiles,
  defaultProfileId,
  selectedId,
  onSelect,
  onChange,
  onAdd,
  onDelete,
  onSetDefault,
}: Props) {
  const selected = selectedId ? profiles[selectedId] : null
  const list = Object.values(profiles)

  return (
    <section className="profiles-panel">
      <div className="profiles-panel__head">
        <h2>Provider Profiles</h2>
        <button type="button" className="profiles-panel__new" onClick={onAdd}>
          <span className="material-symbols-outlined">add_circle</span>
          NEW PROFILE
        </button>
      </div>
      <div className="profiles-panel__body">
        <div className="profiles-panel__list">
          {list.map((p) => (
            <button
              key={p.id}
              type="button"
              className={
                p.id === selectedId
                  ? 'profiles-panel__item is-selected'
                  : 'profiles-panel__item'
              }
              onClick={() => onSelect(p.id)}
            >
              <span className="profiles-panel__item-name">{p.name}</span>
              <span className="profiles-panel__item-meta">
                {p.id === defaultProfileId ? 'Global Default' : p.provider}
              </span>
              {p.id === selectedId && (
                <span className="material-symbols-outlined profiles-panel__check">
                  check_circle
                </span>
              )}
            </button>
          ))}
        </div>
        {selected && (
          <div className="profiles-panel__editor">
            <label>
              Profile Name
              <input
                value={selected.name}
                onChange={(e) => onChange({ ...selected, name: e.target.value })}
              />
            </label>
            <label>
              Provider Type
              <select
                value={selected.provider}
                onChange={(e) => {
                  const provider = e.target.value as Provider
                  const defaults = LLM_CLIENTS.find((c) => c.provider === provider)
                  onChange({
                    ...selected,
                    provider,
                    base_url: defaults?.defaultBaseUrl || selected.base_url,
                  })
                }}
              >
                {LLM_CLIENTS.map((c) => (
                  <option key={c.provider} value={c.provider}>
                    {c.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              API Key
              <input
                type="password"
                value={selected.api_key ?? ''}
                placeholder="sk-…"
                autoComplete="off"
                onChange={(e) =>
                  onChange({ ...selected, api_key: e.target.value || null })
                }
              />
            </label>
            <label>
              Base URL
              <input
                type="url"
                value={selected.base_url}
                onChange={(e) => onChange({ ...selected, base_url: e.target.value })}
              />
            </label>
            <div className="profiles-panel__footer">
              <label className="profiles-panel__default">
                <input
                  type="checkbox"
                  checked={selected.id === defaultProfileId}
                  onChange={() => onSetDefault(selected.id)}
                />
                Set as Global Default
              </label>
              <button
                type="button"
                className="profiles-panel__delete"
                onClick={() => onDelete(selected.id)}
              >
                <span className="material-symbols-outlined">delete</span>
                DELETE PROFILE
              </button>
            </div>
          </div>
        )}
      </div>
    </section>
  )
}
