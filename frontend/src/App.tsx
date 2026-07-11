import { useEffect, useState } from 'react'
import { ConfigApp } from './modules/config-ui'
import { EvaluationsView } from './modules/config-ui/EvaluationsView'
import { ScoutCacheView } from './modules/config-ui/ScoutCacheView'
import { WebCacheView } from './modules/config-ui/WebCacheView'

function currentView() {
  const hash = location.hash.replace(/^#\/?/, '')
  if (hash === 'evaluations') return 'evaluations'
  if (hash === 'cache') return 'cache'
  if (hash === 'web-cache') return 'web-cache'
  return 'config'
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
  return <ConfigApp />
}
