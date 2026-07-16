use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Feature,
    Bugfix,
    Refactor,
    SyncTypes,
}

impl PlanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Bugfix => "bugfix",
            Self::Refactor => "refactor",
            Self::SyncTypes => "sync_types",
        }
    }

    pub fn playbook(self) -> &'static str {
        match self {
            Self::Feature => {
                "Multi-step: impl → module entry if new file → manifest if deps → generate_tests."
            }
            Self::Bugfix => {
                "Minimal patch_file on existing code; no new modules unless unavoidable."
            }
            Self::Refactor => {
                "Behavior-preserving; surgical patches; generate_tests or extend existing tests."
            }
            Self::SyncTypes => "TranspilerAgent + sync_types only; no BuilderAgent code patches.",
        }
    }

    pub fn emit_few_shot(self) -> &'static str {
        match self {
            Self::Feature => {
                r#"## Example feature pipeline shape (structure only)
Step 2 patch_file lib.rs — SEARCH/REPLACE one-line module declare:
<<<<<<< SEARCH
pub mod agent;
=======
pub mod agent;
pub mod config_rate_limit;
>>>>>>> REPLACE

Step 4 patch_file Cargo.toml — single-line manifest hunk:
<<<<<<< SEARCH
axum = "0.7"
=======
axum = { version = "0.7", features = ["macros"] }
>>>>>>> REPLACE
"#
            }
            Self::Bugfix => {
                r#"## Bugfix pipeline shape
- Max 3 steps; no create_file.
- patch_file with grounded SEARCH/REPLACE hunks only.
- End with generate_tests when code changes."#
            }
            Self::Refactor => {
                r#"## Refactor pipeline shape
- No create_file for new source modules.
- patch_file with minimal SEARCH/REPLACE hunks.
- End with generate_tests when code changes."#
            }
            Self::SyncTypes => {
                r#"## sync_types pipeline shape
- TranspilerAgent + sync_types steps only.
"#
            }
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw.to_ascii_lowercase().as_str() {
            "feature" => Ok(Self::Feature),
            "bugfix" => Ok(Self::Bugfix),
            "refactor" => Ok(Self::Refactor),
            "sync_types" => Ok(Self::SyncTypes),
            other => Err(format!(
                "plan_kind must be one of feature, bugfix, refactor, sync_types — got {other:?}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBlueprintArgs {
    pub feature_request: String,
    pub plan_kind: Option<PlanKind>,
    pub expectation: Option<String>,
}

pub fn parse_plan_blueprint_args(args: &Value) -> Result<PlanBlueprintArgs, String> {
    let feature_request = args
        .get("feature_request")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|request| !request.is_empty())
        .ok_or_else(|| "feature_request is required".to_string())?
        .to_string();

    let plan_kind = match args.get("plan_kind") {
        None => None,
        Some(Value::Null) => None,
        Some(value) => {
            let raw = value
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "plan_kind must be a non-empty string".to_string())?;
            Some(PlanKind::parse(raw)?)
        }
    };

    let expectation = args
        .get("expectation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(PlanBlueprintArgs {
        feature_request,
        plan_kind,
        expectation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_requires_feature_request() {
        let err = parse_plan_blueprint_args(&json!({})).unwrap_err();
        assert!(err.contains("feature_request"));
    }

    #[test]
    fn parse_accepts_minimal_args() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "add cache"
        }))
        .expect("parse");
        assert_eq!(args.feature_request, "add cache");
        assert!(args.plan_kind.is_none());
        assert!(args.expectation.is_none());
    }

    #[test]
    fn parse_plan_kind_case_insensitive() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "sync dto",
            "plan_kind": "Sync_Types"
        }))
        .expect("parse");
        assert_eq!(args.plan_kind, Some(PlanKind::SyncTypes));
    }

    #[test]
    fn parse_rejects_unknown_plan_kind() {
        let err = parse_plan_blueprint_args(&json!({
            "feature_request": "x",
            "plan_kind": "hotfix"
        }))
        .unwrap_err();
        assert!(err.contains("plan_kind"));
    }

    #[test]
    fn parse_omits_empty_expectation() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "x",
            "expectation": "   "
        }))
        .expect("parse");
        assert!(args.expectation.is_none());
    }

    #[test]
    fn parse_full_args() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "rate limit",
            "plan_kind": "feature",
            "expectation": "surgical patches only"
        }))
        .expect("parse");
        assert_eq!(args.plan_kind, Some(PlanKind::Feature));
        assert_eq!(args.expectation.as_deref(), Some("surgical patches only"));
    }
}
