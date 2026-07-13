mod common;

use mcp_adjutant::agent::parse_transpile_types_args;
use serde_json::json;

#[test]
fn test_parse_transpile_types_args_valid_input() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "bindings/user.ts",
        "architecture_layout": "typescript"
    });

    let parsed_args = parse_transpile_types_args(&args).expect("Failed to parse arguments");

    assert_eq!(parsed_args.source_paths, vec!["src/models/user.rs"]);
    assert_eq!(parsed_args.target_path, "bindings/user.ts");
    assert_eq!(parsed_args.architecture_layout, "typescript");
    assert!(parsed_args.preserve_paths.is_empty());
    assert!(parsed_args.verify_workspace.is_none());
    assert!(parsed_args.verify_command.is_none());
}

#[test]
fn test_parse_transpile_types_args_missing_source_paths() {
    let args = json!({
        "target_path": "bindings/user.ts",
        "architecture_layout": "typescript"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    // The error message is now "source_paths is required" due to the fix in the source code.
    assert_eq!(
        result.unwrap_err(),
        "source_paths is required".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_empty_source_paths() {
    let args = json!({
        "source_paths": [],
        "target_path": "bindings/user.ts",
        "architecture_layout": "typescript"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "source_paths must not be empty".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_missing_target_path() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "architecture_layout": "typescript"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "target_path is required".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_missing_architecture_layout() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "bindings/user.ts"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "architecture_layout is required".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_with_optional_fields() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "bindings/user.ts",
        "architecture_layout": "typescript",
        "preserve_paths": ["src/foo.rs", "src/bar.rs"],
        "verify_workspace": "/tmp/workspace",
        "verify_command": "npm test"
    });

    let parsed_args = parse_transpile_types_args(&args).expect("Failed to parse arguments");

    assert_eq!(parsed_args.source_paths, vec!["src/models/user.rs"]);
    assert_eq!(parsed_args.target_path, "bindings/user.ts");
    assert_eq!(parsed_args.architecture_layout, "typescript");
    assert_eq!(
        parsed_args.preserve_paths,
        vec!["src/foo.rs", "src/bar.rs"]
    );
    assert_eq!(
        parsed_args.verify_workspace,
        Some("/tmp/workspace".to_string())
    );
    assert_eq!(
        parsed_args.verify_command,
        Some("npm test".to_string())
    );
}

#[test]
fn test_parse_transpile_types_args_with_empty_verify_command() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "bindings/user.ts",
        "architecture_layout": "typescript",
        "verify_command": "  "
    });

    let parsed_args = parse_transpile_types_args(&args).expect("Failed to parse arguments");

    assert!(parsed_args.verify_command.is_none());
}