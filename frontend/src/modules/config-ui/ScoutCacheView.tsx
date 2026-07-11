import { useCallback, useEffect, useState } from 'react'
import { PageShell } from './NavBar'
import { Pager } from './Pager'
import type { ScoutCachePage } from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

const PAGE_SIZE = 20

function formatTimestamp(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString()
}

function preview(text: string, max = 120) {
  return text.length <= max ? text : `${text.slice(0, max)}…`
}

export function ScoutCacheView() {
  const [page, setPage] = useState(1)
  const [data, setData] = useState<ScoutCachePage | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')
  const [expandedInsight, setExpandedInsight] = useState<string | null>(null)

  const load = useCallback((targetPage: number) => {
    setStatus('loading')
    setMessage('')
    fetch(`/api/cache/scout?page=${targetPage}`)
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<ScoutCachePage>
      })
      .then((payload) => {
        setData(payload)
        setPage(payload.page)
        setExpandedInsight(null)
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
    load(page)
  }, [load, page])

  const overview = data?.overview

  return (
    <PageShell
      title="Scout semantic cache"
      subtitle="Vector-backed insight store in .adjutant/cache.db"
      actions={
        <button type="button" className="config-btn" onClick={() => load(page)} disabled={status === 'loading'}>
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
              <span className="stat-card__label">Page size</span>
              <span className="stat-card__value">{PAGE_SIZE}</span>
            </div>
          </section>

          <p className="config-app__meta">
            Project root: <code>{overview.project_root}</code>
          </p>
        </>
      )}

      {status === 'ready' && data && data.total_count === 0 && (
        <p className="config-app__empty">Cache is empty — run scout_context to populate it.</p>
      )}

      {data && data.queries.length > 0 && (
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
              {data.queries.map((row) => (
                <tr key={row.id}>
                  <td>{preview(row.raw_text, 200)}</td>
                  <td>{row.has_embedding ? 'yes' : 'no'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {data && data.insights.length > 0 && (
        <section className="data-panel">
          <h2>Insights</h2>
          <ul className="insight-list">
            {data.insights.map((row) => {
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

      {data && data.code_nodes.length > 0 && (
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
              {data.code_nodes.map((row) => (
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

      {data && data.dependencies.length > 0 && (
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
              {data.dependencies.map((row) => (
                <tr key={`${row.insight_id}:${row.code_node_id}`}>
                  <td><code>{preview(row.insight_id, 40)}</code></td>
                  <td><code>{row.code_node_id}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {data && (
        <Pager
          page={page}
          totalPages={data.total_pages}
          loading={status === 'loading'}
          label="Scout cache pagination"
          onPageChange={setPage}
        />
      )}
    </PageShell>
  )
}
