import { useCallback, useEffect, useState } from 'react'
import { PageShell } from './NavBar'
import type { CacheSnapshot } from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

function formatTimestamp(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString()
}

function preview(text: string, max = 120) {
  return text.length <= max ? text : `${text.slice(0, max)}…`
}

export function ScoutCacheView() {
  const [snapshot, setSnapshot] = useState<CacheSnapshot | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')
  const [expandedInsight, setExpandedInsight] = useState<string | null>(null)

  const load = useCallback(() => {
    setStatus('loading')
    setMessage('')
    fetch('/api/cache')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<CacheSnapshot>
      })
      .then((data) => {
        setSnapshot(data)
        setStatus('ready')
      })
      .catch((error: Error) => {
        emitUiNotify({
          subject: {
            component: 'scout-cache',
            summary: `load failed: ${error.message}`,
          },
          meta: {
              sourceModule: 'config-ui/ScoutCacheView',
            correlationId: null,
          },
        })
        setStatus('error')
        setMessage(error.message)
      })
  }, [])

  useEffect(() => {
    load()
  }, [load])

  const overview = snapshot?.overview

  return (
    <PageShell
      title="Scout semantic cache"
      subtitle="Vector-backed insight store in .adjutant/cache.db"
      actions={
        <button type="button" className="config-btn" onClick={load} disabled={status === 'loading'}>
          {status === 'loading' ? 'Loading…' : 'Refresh'}
        </button>
      }
    >
      {status === 'error' && (
        <p className="config-app__message is-error">Failed to load cache: {message}</p>
      )}

      {overview && (
        <>
          <section className="stats-row">
            <div className="stat-card">
              <span className="stat-card__label">Queries</span>
              <span className="stat-card__value">{overview.query_count}</span>
            </div>
            <div className="stat-card">
              <span className="stat-card__label">Insights</span>
              <span className="stat-card__value">{overview.insight_count}</span>
            </div>
            <div className="stat-card">
              <span className="stat-card__label">Embeddings</span>
              <span className="stat-card__value">{overview.embedding_count}</span>
            </div>
            <div className="stat-card">
              <span className="stat-card__label">Code nodes</span>
              <span className="stat-card__value">{overview.code_node_count}</span>
            </div>
            <div className="stat-card">
              <span className="stat-card__label">Evaluations</span>
              <span className="stat-card__value">{overview.evaluation_count}</span>
            </div>
          </section>

          <p className="config-app__meta">
            Project root: <code>{overview.project_root}</code>
          </p>
        </>
      )}

      {status === 'ready' && snapshot && snapshot.queries.length === 0 && snapshot.insights.length === 0 && (
        <p className="config-app__empty">Cache is empty — scout_context does not populate it yet.</p>
      )}

      {snapshot && snapshot.queries.length > 0 && (
        <section className="data-panel">
          <h2>Queries</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>Query</th>
                <th>Embedding</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.queries.map((row) => (
                <tr key={row.id}>
                  <td>{preview(row.raw_text, 200)}</td>
                  <td>{row.has_embedding ? 'yes' : 'no'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {snapshot && snapshot.insights.length > 0 && (
        <section className="data-panel">
          <h2>Insights</h2>
          <ul className="insight-list">
            {snapshot.insights.map((row) => {
              const isOpen = expandedInsight === row.id
              return (
                <li key={row.id} className="insight-card">
                  <button
                    type="button"
                    className="insight-card__summary"
                    onClick={() => setExpandedInsight(isOpen ? null : row.id)}
                    aria-expanded={isOpen}
                  >
                    <span>{preview(row.query_text ?? row.id, 80)}</span>
                    <span className="eval-card__time">{formatTimestamp(row.created_at)}</span>
                  </button>
                  {isOpen && <pre className="insight-card__content">{row.content}</pre>}
                  {!isOpen && (
                    <p className="insight-card__preview">{preview(row.content)}</p>
                  )}
                </li>
              )
            })}
          </ul>
        </section>
      )}

      {snapshot && snapshot.code_nodes.length > 0 && (
        <section className="data-panel">
          <h2>Code nodes</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>File</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.code_nodes.map((row) => (
                <tr key={row.id}>
                  <td><code>{row.file_path}</code></td>
                  <td>
                    <span className={`status-chip ${row.is_dirty ? 'is-dirty' : 'is-clean'}`}>
                      {row.is_dirty ? 'dirty' : 'clean'}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {snapshot && snapshot.dependencies.length > 0 && (
        <section className="data-panel">
          <h2>Dependencies</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>Insight</th>
                <th>Code node</th>
              </tr>
            </thead>
            <tbody>
              {snapshot.dependencies.map((row) => (
                <tr key={`${row.insight_id}:${row.code_node_id}`}>
                  <td><code>{preview(row.insight_id, 40)}</code></td>
                  <td><code>{row.code_node_id}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      <footer className="config-app__footer-note">
        scout_context does not write to this cache yet; this view shows the designed semantic store.
      </footer>
    </PageShell>
  )
}
