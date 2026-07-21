import { useEffect, useState } from 'react'
import { ConfigApp } from './modules/config-ui'
import { EvaluationsView } from './modules/config-ui/EvaluationsView'
import { MetricsView } from './modules/config-ui/MetricsView'
import { ScoutCacheView } from './modules/config-ui/ScoutCacheView'
import { WebCacheView } from './modules/config-ui/WebCacheView'
import { AppShell } from './modules/config-ui/AppShell'
import './modules/config-ui/config-ui.css'

function currentView() {
  const hash = location.hash.replace(/^#\/?/, '')
  if (hash === 'evaluations') return 'evaluations'
  if (hash === 'cache') return 'cache'
  if (hash === 'web-cache') return 'web-cache'
  if (hash === 'usage') return 'usage'
  if (hash === 'overview') return 'overview'
  if (hash === 'logs') return 'logs'
  return 'config'
}

function StubView({ title, body }: { title: string; body: string }) {
  return (
    <AppShell>
      <header className="config-topbar">
        <div>
          <span className="config-topbar__brand">mcp-adjutant</span>
          <span className="config-topbar__sep">/</span>
          <span className="config-topbar__page">{title}</span>
        </div>
      </header>
      <div className="config-canvas">
        <p className="config-canvas__subtitle">{body}</p>
      </div>
    </AppShell>
  )
}

export default function App() {
  const [view, setView] = useState(currentView)

  useEffect(() => {
    const onHashChange = () => setView(currentView())
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])

  if (view === 'evaluations') return <EvaluationsView />
  if (view === 'cache') return <ScoutCacheView />
  if (view === 'web-cache') return <WebCacheView />
  if (view === 'usage') return <MetricsView />
  if (view === 'overview')
    return (
      <StubView
        title="Overview"
        body="ponytail: status overview stub — use Usage for token metrics."
      />
    )
  if (view === 'logs')
    return (
      <StubView
        title="Logs"
        body="ponytail: logs stub — UI notify stream lives in the MCP host for now."
      />
    )
  return <ConfigApp />
}
