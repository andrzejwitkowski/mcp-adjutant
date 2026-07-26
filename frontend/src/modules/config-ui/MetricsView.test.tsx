import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MetricsView } from './MetricsView'

vi.mock('chart.js', () => {
  class FakeChart {
    destroy(): void {}
    static register(..._args: unknown[]): void {}
  }
  return {
    Chart: FakeChart,
    BarController: class {},
    BarElement: class {},
    CategoryScale: class {},
    Legend: class {},
    LinearScale: class {},
    LineController: class {},
    LineElement: class {},
    PointElement: class {},
    Title: class {},
    Tooltip: class {},
  }
})

vi.mock('./uiLog', () => ({
  emitUiNotify: vi.fn(),
}))

afterEach(cleanup)

describe('MetricsView', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('does not crash when premium_* fields are missing from API payload', async () => {
    // Old API shape without premium bridge columns
    const summary = {
      session_id: 's1',
      utc_date: '2026-07-26',
      prompt_tokens: 10,
      completion_tokens: 5,
      cache_hits: { scout: 0, web_fetcher: 0 },
      by_phase: [
        {
          agent_phase: 'scout',
          prompt_tokens: 10,
          completion_tokens: 5,
          cache_hits: 0,
          job_runs: 1,
        },
      ],
    }

    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify(summary), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify([]), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify([]), { status: 200 }))

    render(<MetricsView />)

    await waitFor(() => {
      expect(screen.getByText('Token usage')).toBeTruthy()
      expect(screen.getByText('scout')).toBeTruthy()
    })
    // Still on page — no white-screen from undefined.toLocaleString()
    expect(screen.getByText('Today bridge in (est.)')).toBeTruthy()
  })
})
