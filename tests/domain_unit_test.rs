#[cfg(test)]
mod tests {
    use mcp_adjutant::domain::{AdjutantConfig, AgentPhase};

    #[test]
    fn test_merge_missing_from_defaults_adds_missing_phases() {
        let mut config = AdjutantConfig::default();
        // Remove a phase to simulate an old config
        config.phases.remove(&AgentPhase::Planner);

        assert!(!config.phases.contains_key(&AgentPhase::Planner));

        config.merge_missing_from_defaults();

        assert!(config.phases.contains_key(&AgentPhase::Planner));
    }
}
