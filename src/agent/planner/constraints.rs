use super::args::{PlanBlueprintArgs, PlanKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorConstraints {
    pub plan_kind: Option<PlanKind>,
    pub expectation: Option<String>,
    pub surgical_patches: bool,
}

fn expects_surgical(expectation: &str) -> bool {
    let lower = expectation.to_ascii_lowercase();
    if lower.contains("not surgical")
        || lower.contains("non-surgical")
        || lower.contains("avoid surgical")
    {
        return false;
    }
    lower.contains("surgical")
}

impl CoordinatorConstraints {
    pub fn none() -> Self {
        Self {
            plan_kind: None,
            expectation: None,
            surgical_patches: false,
        }
    }

    pub fn from_args(args: &PlanBlueprintArgs) -> Self {
        let surgical_patches = matches!(
            args.plan_kind,
            Some(PlanKind::Bugfix) | Some(PlanKind::Refactor)
        ) || args.expectation.as_deref().is_some_and(expects_surgical);
        Self {
            plan_kind: args.plan_kind,
            expectation: args.expectation.clone(),
            surgical_patches,
        }
    }

    pub fn is_default(&self) -> bool {
        self.plan_kind.is_none() && self.expectation.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::super::args::parse_plan_blueprint_args;

    #[test]
    fn none_is_default() {
        assert!(CoordinatorConstraints::none().is_default());
    }

    #[test]
    fn surgical_from_expectation_keyword() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "x",
            "expectation": "Surgical patches only"
        }))
        .expect("parse");
        assert!(CoordinatorConstraints::from_args(&args).surgical_patches);
    }

    #[test]
    fn bugfix_implies_surgical() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "fix",
            "plan_kind": "bugfix"
        }))
        .expect("parse");
        assert!(CoordinatorConstraints::from_args(&args).surgical_patches);
    }

    #[test]
    fn negated_surgical_expectation_is_not_surgical() {
        let args = parse_plan_blueprint_args(&json!({
            "feature_request": "x",
            "expectation": "Not surgical — use create_file for new modules"
        }))
        .expect("parse");
        assert!(!CoordinatorConstraints::from_args(&args).surgical_patches);
    }
}
