mod common;

use mcp_adjutant::agent::parse_transpile_types_args;
use serde_json::json;

#[test]
fn test_parse_transpile_types_args_valid_input() {
    let args = json!({
        "source_paths": ["src/models/user.rs", "src/models/product.rs"],
        "target_path": "target/java/com/example/Models.java",
        "architecture_layout": "java_layout.json",
        "preserve_paths": ["target/java/com/example/BaseModel.java"],
        "verify_workspace": "/tmp/java_project",
        "verify_command": "mvn clean install"
    });

    let parsed_args = parse_transpile_types_args(&args).expect("Failed to parse valid arguments");

    assert_eq!(
        parsed_args.source_paths,
        vec!["src/models/user.rs", "src/models/product.rs"]
    );
    assert_eq!(
        parsed_args.target_path,
        "target/java/com/example/Models.java"
    );
    assert_eq!(parsed_args.architecture_layout, "java_layout.json");
    assert_eq!(
        parsed_args.preserve_paths,
        vec!["target/java/com/example/BaseModel.java"]
    );
    assert_eq!(
        parsed_args.verify_workspace,
        Some("/tmp/java_project".to_string())
    );
    assert_eq!(
        parsed_args.verify_command,
        Some("mvn clean install".to_string())
    );
}

#[test]
fn test_parse_transpile_types_args_missing_source_paths() {
    let args = json!({
        "target_path": "target/java/com/example/Models.java",
        "architecture_layout": "java_layout.json"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "source_paths array is required".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_empty_source_paths() {
    let args = json!({
        "source_paths": [],
        "target_path": "target/java/com/example/Models.java",
        "architecture_layout": "java_layout.json"
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
        "architecture_layout": "java_layout.json"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "target_path is required".to_string());
}

#[test]
fn test_parse_transpile_types_args_missing_architecture_layout() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "target/java/com/example/Models.java"
    });

    let result = parse_transpile_types_args(&args);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err(),
        "architecture_layout is required".to_string()
    );
}

#[test]
fn test_parse_transpile_types_args_optional_fields_missing() {
    let args = json!({
        "source_paths": ["src/models/user.rs"],
        "target_path": "target/java/com/example/Models.java",
        "architecture_layout": "java_layout.json"
    });

    let parsed_args = parse_transpile_types_args(&args).expect("Failed to parse valid arguments");

    assert!(parsed_args.preserve_paths.is_empty());
    assert!(parsed_args.verify_workspace.is_none());
    assert!(parsed_args.verify_command.is_none());
}
