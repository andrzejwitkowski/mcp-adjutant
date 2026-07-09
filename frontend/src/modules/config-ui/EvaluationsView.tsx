import { useCallback, useEffect, useState } from 'react'
import { PageShell } from './NavBar'
import type { AgentEvaluationRow, EvaluationsPage } from './types'
import './config-ui.css'

const PAGE_SIZE = 20

function formatTimestamp(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString()
}

function scoreClass(score: number) {
  if (score >= 8) return 'is-high'
  if (score >= 5) return 'is-mid'
  return 'is-low'
}

export function EvaluationsView() {
  const [page, setPage] = useState(1)
  const [data, setData] = useState<EvaluationsPage | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')
  const [expanded, setExpanded] = useState<string | null>(null)

  const load = useCallback((targetPage: number) => {
    setStatus('loading')
    setMessage('')
    fetch(`/api/evaluations?page=${targetPage}`)
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<EvaluationsPage>
      })
      .then((payload) => {
        setData(payload)
        setPage(payload.page)
        setExpanded(null)
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

  const rows = data?.items ?? []
  const avgScore =
    data?.avg_score != null ? data.avg_score.toFixed(1) : '—'

  return (
    <PageShell
      title="Agent evaluations"
      subtitle="LLM-as-judge ratings from evaluate_agent_performance"
      actions={
        <button
          type="button"
          className="config-btn"
          onClick={() => load(page)}
          disabled={status === 'loading'}
        >
          {status === 'loading' ? 'Loading…' : 'Refresh'}
        </button>
      }
    >
      <section className="stats-row">
        <div className="stat-card">
          <span className="stat-card__label">Total</span>
          <span className="stat-card__value">{data?.total_count ?? 0}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Avg score</span>
          <span className="stat-card__value">{avgScore}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Page size</span>
          <span className="stat-card__value">{PAGE_SIZE}</span>
        </div>
      </section>

      {status === 'error' && (
        <p className="config-app__message is-error">Failed to load evaluations: {message}</p>
      )}

      {status === 'ready' && rows.length === 0 && (
        <p className="config-app__empty">
          No evaluations yet — run <code>evaluate_agent_performance</code> via MCP.
        </p>
      )}

      {rows.length > 0 && (
        <>
          <ul className="eval-list">
            {rows.map((row: AgentEvaluationRow) => {
              const isOpen = expanded === row.id
              return (
                <li key={row.id} className="eval-card">
                  <button
                    type="button"
                    className="eval-card__summary"
                    onClick={() => setExpanded(isOpen ? null : row.id)}
                    aria-expanded={isOpen}
                  >
                    <span className={`score-badge ${scoreClass(row.score)}`}>{row.score}/10</span>
                    <span className="eval-card__agent">{row.agent_name}</span>
                    <span className="eval-card__time">{formatTimestamp(row.created_at)}</span>
                  </button>
                  {isOpen && (
                    <div className="eval-card__detail">
                      <section>
                        <h3>Original task</h3>
                        <pre>{row.original_task}</pre>
                      </section>
                      <section>
                        <h3>Agent output</h3>
                        <pre>{row.agent_output}</pre>
                      </section>
                      <section>
                        <h3>Critique</h3>
                        <pre>{row.feedback_notes}</pre>
                      </section>
                    </div>
                  )}
                </li>
              )
            })}
          </ul>

          {data && data.total_pages > 1 && (
            <nav className="pager" aria-label="Evaluations pagination">
              <button
                type="button"
                className="config-btn"
                disabled={page <= 1 || status === 'loading'}
                onClick={() => setPage((current) => Math.max(1, current - 1))}
              >
                Previous
              </button>
              <span className="pager__status">
                Page {data.page} of {data.total_pages}
              </span>
              <button
                type="button"
                className="config-btn"
                disabled={page >= data.total_pages || status === 'loading'}
                onClick={() => setPage((current) => current + 1)}
              >
                Next
              </button>
            </nav>
          )}
        </>
      )}
    </PageShell>
  )
}
