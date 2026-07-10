mod discovery;
mod llm_tools;
mod targets;

pub use discovery::find_refactor_targets;
pub use llm_tools::transformer_tool_set;
pub use targets::{
    filter_targets_by_scope, format_refactor_targets_block, parse_apply_structural_codemod_arguments,
    parse_method_name, path_under_scope, RefactorTarget, TargetLineRange,
};

pub fn extract_refactor_instruction(prompt: &str) -> String {
    prompt
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("Refactor instruction:")
                .or_else(|| trimmed.strip_prefix("Instruction:"))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| prompt.to_string())
}

pub fn build_scout_refactor_query(method_name: &str) -> String {
    format!(
        "PHASE_1_SCOUT\n\nFind all call sites for `{method_name}` across the repository.\n\n\
         Use ripgrep to locate candidate files, then ast_calls for function/method calls and \
         ast_constructions for struct/type construction literals. read_file small slices when needed. \
         Finish with a finalize report listing each file_path and line numbers or ranges in a machine-readable block:\n\
         ```refactor_targets\n[{{\"file_path\":\"...\",\"lines\":[...]}}]\n```\n\
         or for multi-line struct literals:\n\
         ```refactor_targets\n[{{\"file_path\":\"...\",\"ranges\":[{{\"start\":25,\"end\":32}}]}}]\n```"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::resolve_workspace_path;
    use serde_json::json;
    use targets::parse_refactor_targets_json;

    #[test]
    fn transformer_tool_set_registers_all_tools() {
        let tools = transformer_tool_set();
        let names: Vec<_> = tools
            .definitions()
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "gather_refactor_targets".to_string(),
                "apply_structural_codemod".to_string(),
            ]
        );
    }

    #[test]
    fn build_scout_refactor_query_mentions_ast_tools() {
        let query = build_scout_refactor_query("validate");
        assert!(query.contains("PHASE_1_SCOUT"));
        assert!(query.contains("validate"));
        assert!(query.contains("ast_calls"));
        assert!(query.contains("ast_constructions"));
        assert!(query.contains("refactor_targets"));
        assert!(query.contains("ranges"));
    }

    #[test]
    fn parse_refactor_targets_json_accepts_valid_payload() {
        let targets = parse_refactor_targets_json(
            r#"[{"file_path":"src/a.rs","lines":[1,3]}]"#,
        )
        .expect("parse");

        assert_eq!(targets.len(), 1);
        assert!(targets[0].file_path.ends_with("src/a.rs"));
        assert_eq!(targets[0].lines, vec![1, 3]);
    }

    #[test]
    fn parse_refactor_targets_json_accepts_ranges() {
        let targets = parse_refactor_targets_json(
            r#"[{"file_path":"src/a.rs","ranges":[{"start":25,"end":32}]}]"#,
        )
        .expect("parse");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ranges, vec![TargetLineRange { start: 25, end: 32 }]);
        assert!(targets[0].lines.is_empty());
    }

    #[test]
    fn parse_refactor_targets_json_rejects_invalid_payload() {
        assert!(parse_refactor_targets_json("not json").is_err());
        assert!(parse_refactor_targets_json(r#"[{"lines":[1]}]"#).is_err());
        assert!(parse_refactor_targets_json(r#"[{"file_path":"x.rs","lines":[]}]"#).is_err());
        assert!(parse_refactor_targets_json(
            r#"[{"file_path":"x.rs","ranges":[{"start":5,"end":2}]}]"#
        )
        .is_err());
    }

    #[test]
    fn parse_apply_structural_codemod_arguments_extracts_fields() {
        let (rule, targets) = parse_apply_structural_codemod_arguments(&json!({
            "transformation_rule": "Add true as argument",
            "refactor_targets_json": r#"[{"file_path":"src/b.rs","lines":[2]}]"#,
        }))
        .expect("args");

        assert_eq!(rule, "Add true as argument");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].lines, vec![2]);
    }

    #[test]
    fn path_under_scope_respects_directory_boundary() {
        let scope = resolve_workspace_path("scripts/pkg_a");
        assert!(path_under_scope(
            &resolve_workspace_path("scripts/pkg_a/foo.py"),
            &scope
        ));
        assert!(!path_under_scope(
            &resolve_workspace_path("scripts/pkg_b/foo.py"),
            &scope
        ));
    }

    #[test]
    fn extract_refactor_instruction_reads_labeled_line() {
        let prompt = "PHASE_3_5_TRANSFORMER\nRefactor instruction: rename headline to subject\n";
        assert_eq!(
            extract_refactor_instruction(prompt),
            "rename headline to subject"
        );
    }

    #[test]
    fn format_refactor_targets_block_emits_json_fence() {
        let block = format_refactor_targets_block(&[RefactorTarget {
            file_path: std::path::PathBuf::from("src/a.rs"),
            lines: Vec::new(),
            ranges: vec![TargetLineRange { start: 10, end: 15 }],
        }]);
        assert!(block.contains("```refactor_targets"));
        assert!(block.contains(r#""start":10"#));
    }
}
