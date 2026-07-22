import { describe, expect, it } from 'vitest'
import { AGENT_PHASES, applyBindingToAllPhases } from './agents'
import type { AdjutantConfig, PhaseBinding } from './types'

describe('applyBindingToAllPhases', () => {
  it('copies binding onto every AGENT_PHASES entry', () => {
    const phases: AdjutantConfig['phases'] = {
      scout: {
        profile_id: 'old',
        model_name: 'old-model',
        max_tokens: 256,
        temperature: 0,
      },
    }
    const binding: PhaseBinding = {
      profile_id: 'test_profile',
      model_name: 'gpt-4',
      max_tokens: 1000,
      temperature: 0.7,
    }

    const result = applyBindingToAllPhases(phases, binding)

    for (const { phase } of AGENT_PHASES) {
      expect(result[phase]).toEqual(binding)
      expect(result[phase]).not.toBe(binding)
    }
  })
})
