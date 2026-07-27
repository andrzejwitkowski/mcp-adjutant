import { useCallback, useEffect, useState } from 'react'
import { PageShell } from './NavBar'
import type { LogEntry, LogLevel } from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

function formatTimestamp(ms: number) {
  return new Date(ms).toLocaleString()
}

function levelClass(level: LogLevel) {
  return level === 'warn' ? 'is-mid' : 'is-low'
}

export function LogsView() {
  const [entries, setEntries] = useState<LogEntry[]>([])
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')

  const load = useCallback(() => {
    setStatus('loading')
    setMessage('')
    fetch('/api/logs')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json() as Promise<LogEntry[]>
      })
      .then((payload) => {
        setEntries(payload)
        setStatus('ready')
      })
      .catch((error: Error) => {
        emitUiNotify({
          subject: { component: 'logs', summary: `load failed: ${error.message}` },
          meta: { sourceModule: 'config-ui/LogsView', correlationId: null },
        })
        setStatus('error')
        setMessage(error.message)
      })
  }, [])

  useEffect(() => {
    load()
  }, [load])

  return (
    <PageShell
      title="Logs"
      subtitle="Session ERROR / WARN / panic — in-memory, newest first"
      actions={
        <button
          type="button"
          className="config-btn"
          onClick={load}
          disabled={status === 'loading'}
        >
          {status === 'loading' ? 'Loading…' : 'Refresh'}
        </button>
      }
    >
      {status === 'error' && (
        <p className="config-app__message is-error">Failed to load logs: {message}</p>
      )}

      {status === 'ready' && entries.length === 0 && (
        <p className="config-app__empty">No errors yet.</p>
      )}

      {entries.length > 0 && (
        <ul className="log-list">
          {entries.map((row, index) => (
            <li key={`${row.ts_unix_ms}-${index}`} className="log-row">
              <div className="log-row__meta">
                <span className={`score-badge ${levelClass(row.level)}`}>{row.level}</span>
                <span className="log-row__source">{row.source}</span>
                <span className="log-row__time">{formatTimestamp(row.ts_unix_ms)}</span>
              </div>
              <pre className="log-row__msg">{row.message}</pre>
            </li>
          ))}
        </ul>
      )}
    </PageShell>
  )
}
