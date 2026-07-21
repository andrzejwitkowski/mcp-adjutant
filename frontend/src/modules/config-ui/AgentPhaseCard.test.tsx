import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { AgentPhaseCard } from './AgentPhaseCard'

describe('AgentPhaseCard', () => {
  it('renders title', () => {
    render(
      <AgentPhaseCard
        id="builder"
        title="Builder"
        hint="tests"
        icon="B"
        binding={{ profile_id: 'default', model_name: 'm', max_tokens: 1, temperature: 0 }}
        profiles={{}}
        onChange={() => {}}
      />,
    )
    expect(screen.getByText('Builder')).toBeTruthy()
  })
})
