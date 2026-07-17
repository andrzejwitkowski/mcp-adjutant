/// Legacy `agent_evaluations.agent_name` values → canonical phase names.
const ALIASES: &[(&str, &str)] = &[
    ("scout", "Phase_1_Scout"),
    ("scoutagent", "Phase_1_Scout"),
    ("phase_1_scout", "Phase_1_Scout"),
    ("Scout", "Phase_1_Scout"),
    ("ScoutAgent", "Phase_1_Scout"),
    ("builder", "Phase_4_Builder"),
    ("builderagent", "Phase_4_Builder"),
    ("phase_4_builder", "Phase_4_Builder"),
    ("Builder", "Phase_4_Builder"),
    ("BuilderAgent", "Phase_4_Builder"),
    ("triage", "Phase_5_Triage"),
    ("triageagent", "Phase_5_Triage"),
    ("phase_5_triage", "Phase_5_Triage"),
    ("Triage", "Phase_5_Triage"),
    ("TriageAgent", "Phase_5_Triage"),
    ("planner", "PlannerAgent"),
    ("planneragent", "PlannerAgent"),
    ("Planner", "PlannerAgent"),
    ("transpiler", "TranspilerAgent"),
    ("transpileragent", "TranspilerAgent"),
    ("phase_3_transformer", "TranspilerAgent"),
    ("evaluator", "EvaluatorAgent"),
    ("evaluator_agent", "EvaluatorAgent"),
];

pub fn normalize_agent_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    for (alias, canonical) in ALIASES {
        if lower == *alias || trimmed == *alias {
            return (*canonical).to_string();
        }
    }
    trimmed.to_string()
}

pub fn backfill_evaluation_agent_names(conn: &rusqlite::Connection) -> Result<(), String> {
    let legacy: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_evaluations WHERE agent_name IN (
                'builder','BuilderAgent','Builder','Scout','ScoutAgent',
                'Triage','TriageAgent','Planner','planner','scout','triage'
            )",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if legacy == 0 {
        return Ok(());
    }

    let mut seen = std::collections::HashSet::new();
    for (from, to) in ALIASES {
        if *from == *to || !seen.insert(from) {
            continue;
        }
        conn.execute(
            "UPDATE agent_evaluations SET agent_name = ?1 WHERE agent_name = ?2",
            rusqlite::params![to, from],
        )
        .map_err(|err| format!("failed to backfill agent_name {from:?}: {err}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_maps_legacy_builder_aliases() {
        assert_eq!(normalize_agent_name("builder"), "Phase_4_Builder");
        assert_eq!(normalize_agent_name("BuilderAgent"), "Phase_4_Builder");
    }

    #[test]
    fn normalize_preserves_unknown_names() {
        assert_eq!(normalize_agent_name("CustomAgent"), "CustomAgent");
    }
}
