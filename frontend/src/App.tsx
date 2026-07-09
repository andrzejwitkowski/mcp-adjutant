import { useEffect, useState } from 'react'
import { ConfigApp } from './modules/config-ui'
import { EvaluationsView } from './modules/config-ui/EvaluationsView'
import { ScoutCacheView } from './modules/config-ui/ScoutCacheView'

function currentView() {
  const hash = location.hash.replace(/^#\/?/, '')
  if (hash === 'evaluations') return 'evaluations'
  if (hash === 'cache') return 'cache'
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
  return <ConfigApp />
}
