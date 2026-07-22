import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AgentPhaseCard } from './AgentPhaseCard'
import type { ProviderProfile, WebFetcherProfile } from './types'

afterEach(cleanup)

describe('AgentPhaseCard', () => {
  const profiles: Record<string, ProviderProfile> = {
    'profile-1': {
      id: 'profile-1',
      name: 'Profile 1',
      provider: 'open_ai',
      api_key: null,
      base_url: 'https://api.openai.com/v1',
    },
    'profile-2': {
      id: 'profile-2',
      name: 'Profile 2',
      provider: 'open_ai',
      api_key: null,
      base_url: 'https://api.openai.com/v1',
    },
  }

  const defaultProps = {
    id: 'test-id',
    title: 'Test Title',
    hint: 'Test Hint',
    icon: 'settings',
    binding: {
      profile_id: 'profile-1',
      model_name: 'gpt-4',
      max_tokens: 1024,
      temperature: 0.7,
    },
    profiles,
    onChange: vi.fn(),
  }

  it('renders title and hint', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    expect(screen.getByText('Test Title')).toBeTruthy()
    expect(screen.getByText('Test Hint')).toBeTruthy()
  })

  it('calls onChange when profile is changed', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'profile-2' } })
    expect(defaultProps.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ profile_id: 'profile-2' }),
    )
  })

  it('calls onChange when model name is changed', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'claude-3' } })
    expect(defaultProps.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ model_name: 'claude-3' }),
    )
  })

  it('calls onChange when temperature is changed', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    fireEvent.change(screen.getByRole('slider'), { target: { value: '1.5' } })
    expect(defaultProps.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ temperature: 1.5 }),
    )
  })

  it('calls onChange when max tokens is changed', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2048' } })
    expect(defaultProps.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ max_tokens: 2048 }),
    )
  })

  it('calls onApplyToAll when Apply to all is clicked', () => {
    const onApplyToAll = vi.fn()
    render(<AgentPhaseCard {...defaultProps} onApplyToAll={onApplyToAll} />)
    fireEvent.click(screen.getByRole('button', { name: /Apply to all/i }))
    expect(onApplyToAll).toHaveBeenCalledOnce()
  })

  it('hides Apply to all when onApplyToAll is omitted', () => {
    render(<AgentPhaseCard {...defaultProps} />)
    expect(screen.queryByRole('button', { name: /Apply to all/i })).toBeNull()
  })

  it('renders web fetcher tunables when provided', () => {
    const webFetcher: WebFetcherProfile = {
      brave_api_key: null,
      max_search_hops: 3,
      token_budget: 8000,
      cache_ttl_seconds: 604800,
      web_cache_threshold: 0.78,
    }
    render(
      <AgentPhaseCard
        {...defaultProps}
        webFetcher={webFetcher}
        onWebFetcherChange={vi.fn()}
      />,
    )
    expect(screen.getByText(/Web Fetcher tunables/i)).toBeTruthy()
  })
})
