import type { ReactNode } from 'react'
import { AGENT_PHASES } from './agents'

const LINKS = [
  { hash: '#/overview', label: 'Overview', icon: 'dashboard' },
  { hash: '#/', label: 'LLM Config', icon: 'tune' },
  { hash: '#/evaluations', label: 'Evaluations', icon: 'assessment' },
  { hash: '#/cache', label: 'Scout cache', icon: 'database' },
  { hash: '#/web-cache', label: 'Web cache', icon: 'language' },
  { hash: '#/usage', label: 'Usage', icon: 'monitoring' },
  { hash: '#/logs', label: 'Logs', icon: 'terminal' },
] as const

function currentHash() {
  return location.hash || '#/'
}

function isConfigView(hash: string) {
  return hash === '#/' || hash === '#' || hash === ''
}

export function AppShell({ children }: { children: ReactNode }) {
  const current = currentHash()
  const onConfig = isConfigView(current)

  return (
    <div className="app-shell">
      <aside className="app-shell__nav" aria-label="Main">
        <div className="app-shell__brand">
          <span className="material-symbols-outlined app-shell__brand-icon">terminal</span>
          <div>
            <div className="app-shell__brand-title">mcp-adjutant</div>
            <div className="app-shell__brand-sub">LLM Orchestrator</div>
          </div>
        </div>
        <nav className="app-shell__links">
          {LINKS.map(({ hash, label, icon }) => (
            <a
              key={hash}
              href={hash}
              className={
                current === hash || (hash === '#/' && onConfig)
                  ? 'app-shell__link is-active'
                  : 'app-shell__link'
              }
            >
              <span className="material-symbols-outlined">{icon}</span>
              {label}
            </a>
          ))}
        </nav>
        {onConfig && (
          <div className="app-shell__agents">
            <div className="app-shell__agents-label">Agents</div>
            {AGENT_PHASES.map(({ phase, title, icon }) => (
              <a
                key={phase}
                href={`#agent-${phase}`}
                className="app-shell__agent-link"
                onClick={(e) => {
                  e.preventDefault()
                  document
                    .getElementById(`agent-${phase}`)
                    ?.scrollIntoView({ behavior: 'smooth', block: 'start' })
                }}
              >
                <span className="material-symbols-outlined">{icon}</span>
                {title}
              </a>
            ))}
          </div>
        )}
      </aside>
      <div className="app-shell__main">{children}</div>
    </div>
  )
}
