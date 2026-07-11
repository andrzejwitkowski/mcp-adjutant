import { useCallback, useEffect, useState } from 'react'
import { PageShell } from './NavBar'
import { Pager } from './Pager'
import type { WebCachePage } from './types'
import './config-ui.css'

const PAGE_SIZE = 20

function formatTimestamp(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString()
}

function preview(text: string, max = 120) {
  return text.length <= max ? text : `${text.slice(0, max)}…`
}

export function WebCacheView() {
  const [page, setPage] = useState(1)
  const [data, setData] = useState<WebCachePage | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')
  const [expandedReport, setExpandedReport] = useState<string | null>(null)

  const load = useCallback((targetPage: number) => {
    setStatus('loading')
    setMessage('')
    fetch(`/api/cache/web?page=${targetPage}`)
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<WebCachePage>
      })
      .then((payload) => {
        setData(payload)
        setPage(payload.page)
        setExpandedReport(null)
        setStatus('ready')
      })
      .catch((error: Error) => {
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
      title="Web fetcher cache"
      subtitle="Vector-backed web research store in .adjutant/cache.db"
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
        <section className="stats-row">
          <div className="stat-card">
            <span className="stat-card__label">Web queries</span>
            <span className="stat-card__value">{overview.web_query_count}</span>
          </div>
          <div className="stat-card">
            <span className="stat-card__label">Web reports</span>
            <span className="stat-card__value">{overview.web_report_count}</span>
          </div>
          <div className="stat-card">
            <span className="stat-card__label">Web sources</span>
            <span className="stat-card__value">{overview.web_source_count}</span>
          </div>
          <div className="stat-card">
            <span className="stat-card__label">Dependencies</span>
            <span className="stat-card__value">{overview.web_dependency_count}</span>
          </div>
          <div className="stat-card">
            <span className="stat-card__label">Page size</span>
            <span className="stat-card__value">{PAGE_SIZE}</span>
          </div>
        </section>
      )}

      {status === 'ready' && data && data.total_count === 0 && (
        <p className="config-app__empty">Web cache is empty — run web_fetch to populate it.</p>
      )}

      {data && data.web_queries.length > 0 && (
        <section className="data-panel">
          <h2>Web queries</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>Query</th>
                <th>Embedding</th>
              </tr>
            </thead>
            <tbody>
              {data.web_queries.map((row) => (
                <tr key={row.id}>
                  <td>{preview(row.raw_text, 200)}</td>
                  <td>{row.has_embedding ? 'yes' : 'no'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {data && data.web_reports.length > 0 && (
        <section className="data-panel">
          <h2>Web reports</h2>
          <ul className="insight-list">
            {data.web_reports.map((row) => {
              const isOpen = expandedReport === row.id
              return (
                <li key={row.id} className="insight-card">
                  <button
                    type="button"
                    className="insight-card__summary"
                    onClick={() => setExpandedReport(isOpen ? null : row.id)}
                    aria-expanded={isOpen}
                  >
                    <span>{preview(row.query_text ?? row.id, 80)}</span>
                    <span className="eval-card__time">{formatTimestamp(row.created_at)}</span>
                  </button>
                  {isOpen && <pre className="insight-card__content">{row.content}</pre>}
                  {!isOpen && <p className="insight-card__preview">{preview(row.content)}</p>}
                </li>
              )
            })}
          </ul>
        </section>
      )}

      {data && data.web_sources.length > 0 && (
        <section className="data-panel">
          <h2>Web sources</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>URL</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {data.web_sources.map((row) => (
                <tr key={row.id}>
                  <td>
                    <a href={row.url} target="_blank" rel="noreferrer">
                      {preview(row.url, 80)}
                    </a>
                  </td>
                  <td>
                    <span className={`status-chip ${row.is_stale ? 'is-dirty' : 'is-clean'}`}>
                      {row.is_stale ? 'stale' : 'fresh'}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {data && data.web_dependencies.length > 0 && (
        <section className="data-panel">
          <h2>Dependencies</h2>
          <table className="data-table">
            <thead>
              <tr>
                <th>Report</th>
                <th>Source</th>
              </tr>
            </thead>
            <tbody>
              {data.web_dependencies.map((row) => (
                <tr key={`${row.report_id}:${row.source_id}`}>
                  <td><code>{preview(row.report_id, 40)}</code></td>
                  <td><code>{preview(row.source_id, 40)}</code></td>
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
          label="Web cache pagination"
          onPageChange={setPage}
        />
      )}
    </PageShell>
  )
}
