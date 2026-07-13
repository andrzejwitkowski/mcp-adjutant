import {
  BarController,
  BarElement,
  CategoryScale,
  Chart,
  Legend,
  LinearScale,
  LineController,
  LineElement,
  PointElement,
  Title,
  Tooltip,
} from 'chart.js'
import { useCallback, useEffect, useRef, useState } from 'react'
import { PageShell } from './NavBar'
import type { DailyMetricsRow, MetricsSummary, TimelineBucket } from './types'
import { emitUiNotify } from './uiLog'
import './config-ui.css'

Chart.register(
  CategoryScale,
  LinearScale,
  BarElement,
  BarController,
  LineController,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
)

const PHASE_COLORS: Record<string, string> = {
  scout: '#4f8cff',
  triage: '#f5a623',
  builder: '#50c878',
  transformer: '#b084cc',
  evaluator: '#ff6b6b',
  log_analyzer: '#e85d75',
  web_fetcher: '#20b2aa',
}

function phaseColor(phase: string) {
  return PHASE_COLORS[phase] ?? '#8884d8'
}

function utcToday() {
  return new Date().toISOString().slice(0, 10)
}

function utcDaysAgo(days: number) {
  const d = new Date()
  d.setUTCDate(d.getUTCDate() - days)
  return d.toISOString().slice(0, 10)
}

function formatPhase(label: string) {
  return label.replace(/_/g, ' ')
}

export function MetricsView() {
  const [fromDate, setFromDate] = useState(utcDaysAgo(7))
  const [toDate, setToDate] = useState(utcToday())
  const [timelineDate, setTimelineDate] = useState(utcToday())
  const [summary, setSummary] = useState<MetricsSummary | null>(null)
  const [daily, setDaily] = useState<DailyMetricsRow[]>([])
  const [timeline, setTimeline] = useState<TimelineBucket[]>([])
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [message, setMessage] = useState('')

  const barRef = useRef<HTMLCanvasElement>(null)
  const lineRef = useRef<HTMLCanvasElement>(null)
  const barChart = useRef<Chart | null>(null)
  const lineChart = useRef<Chart | null>(null)

  const load = useCallback(() => {
    setStatus('loading')
    setMessage('')
    Promise.all([
      fetch('/api/metrics/summary').then((r) => {
        if (!r.ok) throw new Error(`summary HTTP ${r.status}`)
        return r.json() as Promise<MetricsSummary>
      }),
      fetch(`/api/metrics/daily?from=${fromDate}&to=${toDate}`).then((r) => {
        if (!r.ok) throw new Error(`daily HTTP ${r.status}`)
        return r.json() as Promise<DailyMetricsRow[]>
      }),
      fetch(`/api/metrics/timeline?date=${timelineDate}`).then((r) => {
        if (!r.ok) throw new Error(`timeline HTTP ${r.status}`)
        return r.json() as Promise<TimelineBucket[]>
      }),
    ])
      .then(([summaryPayload, dailyPayload, timelinePayload]) => {
        setSummary(summaryPayload)
        setDaily(dailyPayload)
        setTimeline(timelinePayload)
        setStatus('ready')
      })
      .catch((error: Error) => {
        emitUiNotify({
          subject: { component: 'metrics', summary: `load failed: ${error.message}` },
          meta: { sourceModule: 'config-ui/MetricsView', correlationId: null },
        })
        setStatus('error')
        setMessage(error.message)
      })
  }, [fromDate, toDate, timelineDate])

  useEffect(() => {
    load()
  }, [load])

  useEffect(() => {
    if (status !== 'ready' || !barRef.current) return

    const dates = [...new Set(daily.map((row) => row.date))].sort()
    const phases = [...new Set(daily.map((row) => row.agent_phase))].sort()

    const datasets = phases.map((phase) => ({
      label: formatPhase(phase),
      data: dates.map((date) => {
        const row = daily.find((r) => r.date === date && r.agent_phase === phase)
        return (row?.prompt_tokens ?? 0) + (row?.completion_tokens ?? 0)
      }),
      backgroundColor: phaseColor(phase),
      stack: 'tokens',
    }))

    barChart.current?.destroy()
    barChart.current = new Chart(barRef.current, {
      type: 'bar',
      data: { labels: dates, datasets },
      options: {
        responsive: true,
        plugins: {
          title: { display: true, text: 'Daily token usage by agent (UTC)' },
          legend: { position: 'bottom' },
        },
        scales: {
          x: { stacked: true },
          y: { stacked: true, beginAtZero: true, title: { display: true, text: 'Tokens' } },
        },
      },
    })

    return () => {
      barChart.current?.destroy()
      barChart.current = null
    }
  }, [daily, status])

  useEffect(() => {
    if (status !== 'ready' || !lineRef.current) return

    const phases = [...new Set(timeline.map((row) => row.agent_phase))].sort()
    const hours = [...new Set(timeline.map((row) => row.hour))].sort((a, b) => a - b)
    const labels = hours.map((h) => `${String(h).padStart(2, '0')}:00`)

    const datasets = phases.map((phase) => ({
      label: formatPhase(phase),
      data: hours.map((hour) => {
        const rows = timeline.filter((r) => r.agent_phase === phase && r.hour <= hour)
        const last = rows.sort((a, b) => a.hour - b.hour).at(-1)
        return last
          ? last.cumulative_prompt_tokens + last.cumulative_completion_tokens
          : 0
      }),
      borderColor: phaseColor(phase),
      backgroundColor: phaseColor(phase),
      tension: 0.2,
      fill: false,
    }))

    lineChart.current?.destroy()
    lineChart.current = new Chart(lineRef.current, {
      type: 'line',
      data: { labels, datasets },
      options: {
        responsive: true,
        plugins: {
          title: {
            display: true,
            text: `Cumulative tokens on ${timelineDate} (UTC)`,
          },
          legend: { position: 'bottom' },
        },
        scales: {
          y: { beginAtZero: true, title: { display: true, text: 'Cumulative tokens' } },
        },
      },
    })

    return () => {
      lineChart.current?.destroy()
      lineChart.current = null
    }
  }, [timeline, timelineDate, status])

  const totalInput = summary?.prompt_tokens ?? 0
  const totalOutput = summary?.completion_tokens ?? 0

  return (
    <PageShell
      title="Token usage"
      subtitle="LLM input/output tokens and cache hits — global metrics (UTC days)"
      actions={
        <button type="button" className="config-btn" onClick={load} disabled={status === 'loading'}>
          {status === 'loading' ? 'Loading…' : 'Refresh'}
        </button>
      }
    >
      <section className="stats-row">
        <div className="stat-card">
          <span className="stat-card__label">Today input</span>
          <span className="stat-card__value">{totalInput.toLocaleString()}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Today output</span>
          <span className="stat-card__value">{totalOutput.toLocaleString()}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Scout cache hits</span>
          <span className="stat-card__value">{summary?.cache_hits.scout ?? 0}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Web cache hits</span>
          <span className="stat-card__value">{summary?.cache_hits.web_fetcher ?? 0}</span>
        </div>
        <div className="stat-card">
          <span className="stat-card__label">Session</span>
          <span className="stat-card__value stat-card__value--small">
            {summary?.session_id ?? '—'}
          </span>
        </div>
      </section>

      <section className="metrics-filters">
        <label>
          From (UTC)
          <input
            type="date"
            value={fromDate}
            onChange={(e) => setFromDate(e.target.value)}
          />
        </label>
        <label>
          To (UTC)
          <input type="date" value={toDate} onChange={(e) => setToDate(e.target.value)} />
        </label>
        <label>
          Timeline day (UTC)
          <input
            type="date"
            value={timelineDate}
            onChange={(e) => setTimelineDate(e.target.value)}
          />
        </label>
      </section>

      {status === 'error' && (
        <p className="config-app__message is-error">Failed to load metrics: {message}</p>
      )}

      {status === 'ready' && summary && summary.by_phase.length > 0 && (
        <section className="metrics-phase-table">
          <h3>Today by agent (UTC)</h3>
          <table className="data-table">
            <thead>
              <tr>
                <th>Agent</th>
                <th>Runs</th>
                <th>Input tokens</th>
                <th>Output tokens</th>
              </tr>
            </thead>
            <tbody>
              {summary.by_phase.map((row) => (
                <tr key={row.agent_phase}>
                  <td>
                    <span
                      className="metrics-phase-dot"
                      style={{ backgroundColor: phaseColor(row.agent_phase) }}
                    />
                    {formatPhase(row.agent_phase)}
                  </td>
                  <td>{row.job_runs}</td>
                  <td>{row.prompt_tokens.toLocaleString()}</td>
                  <td>{row.completion_tokens.toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {status === 'ready' && daily.length === 0 && timeline.length === 0 && (
        <p className="config-app__empty">
          No token usage recorded yet. Run an agent job to populate metrics.
        </p>
      )}

      <div className="metrics-charts">
        <div className="metrics-chart">
          <canvas ref={barRef} role="img" aria-label="Daily token usage chart" />
        </div>
        <div className="metrics-chart">
          <canvas ref={lineRef} role="img" aria-label="Cumulative token timeline chart" />
        </div>
      </div>
    </PageShell>
  )
}
