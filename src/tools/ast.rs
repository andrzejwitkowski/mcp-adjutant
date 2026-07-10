use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use super::lang::{detect_file_language, SourceLanguage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

pub struct AstUsageFinder;

impl AstUsageFinder {
    pub fn find_calls_in_file(file_path: &Path, method_name: &str) -> Result<Vec<usize>, String> {
        let report = detect_file_language(file_path)?;
        if report.language == SourceLanguage::Unknown {
            return Err(format!(
                "cannot determine language for AST scan: {}",
                file_path.display()
            ));
        }

        let (language, query_source) = call_grammar_for_language(report.language)?;

        let source = std::fs::read_to_string(file_path)
            .map_err(|err| format!("failed to read {}: {err}", file_path.display()))?;

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|err| format!("failed to set parser language: {err}"))?;

        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| format!("failed to parse {}", file_path.display()))?;

        let query = Query::new(&language, query_source)
            .map_err(|err| format!("failed to compile tree-sitter query: {err}"))?;

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

        let mut lines = Vec::new();
        while let Some(query_match) = matches.next() {
            for capture in query_match.captures {
                let name = query.capture_names()[capture.index as usize];
                if name != "name" {
                    continue;
                }

                let node = capture.node;
                let captured = node
                    .utf8_text(source.as_bytes())
                    .map_err(|err| format!("invalid utf8 in capture: {err}"))?;

                if captured != method_name {
                    continue;
                }

                let line = node.start_position().row + 1;
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }

        lines.sort_unstable();
        Ok(lines)
    }

    pub fn find_call_expression_ranges_in_file(
        file_path: &Path,
        method_name: &str,
    ) -> Result<Vec<LineRange>, String> {
        let report = detect_file_language(file_path)?;
        if report.language == SourceLanguage::Unknown {
            return Err(format!(
                "cannot determine language for AST scan: {}",
                file_path.display()
            ));
        }

        let (language, query_source) = call_grammar_for_language(report.language)?;
        let source = std::fs::read_to_string(file_path)
            .map_err(|err| format!("failed to read {}: {err}", file_path.display()))?;

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|err| format!("failed to set parser language: {err}"))?;

        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| format!("failed to parse {}", file_path.display()))?;

        let query = Query::new(&language, query_source)
            .map_err(|err| format!("failed to compile tree-sitter query: {err}"))?;

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        let mut ranges = Vec::new();

        while let Some(query_match) = matches.next() {
            for capture in query_match.captures {
                if query.capture_names()[capture.index as usize] != "name" {
                    continue;
                }
                let node = capture.node;
                let captured = node
                    .utf8_text(source.as_bytes())
                    .map_err(|err| format!("invalid utf8 in capture: {err}"))?;
                if captured != method_name {
                    continue;
                }
                let range = enclosing_call_range(node);
                if !ranges.iter().any(|existing: &LineRange| {
                    existing.start == range.start && existing.end == range.end
                }) {
                    ranges.push(range);
                }
            }
        }

        ranges.sort_by_key(|range| (range.start, range.end));
        Ok(ranges)
    }

    pub fn find_construction_sites_in_file(
        file_path: &Path,
        type_name: &str,
    ) -> Result<Vec<LineRange>, String> {
        let report = detect_file_language(file_path)?;
        if report.language == SourceLanguage::Unknown {
            return Err(format!(
                "cannot determine language for AST scan: {}",
                file_path.display()
            ));
        }

        if report.language == SourceLanguage::Sql {
            return Ok(Vec::new());
        }

        if report.language == SourceLanguage::Python {
            let lines = Self::find_calls_in_file(file_path, type_name)?;
            return Ok(lines
                .into_iter()
                .map(|line| LineRange {
                    start: line,
                    end: line,
                })
                .collect());
        }

        let (language, query_source) = construction_grammar_for_language(report.language)?;

        let source = std::fs::read_to_string(file_path)
            .map_err(|err| format!("failed to read {}: {err}", file_path.display()))?;

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|err| format!("failed to set parser language: {err}"))?;

        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| format!("failed to parse {}", file_path.display()))?;

        let query = Query::new(&language, query_source)
            .map_err(|err| format!("failed to compile tree-sitter query: {err}"))?;

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

        let mut ranges = Vec::new();
        while let Some(query_match) = matches.next() {
            let mut matched_type = false;
            let mut site_node: Option<Node> = None;

            for capture in query_match.captures {
                let name = query.capture_names()[capture.index as usize];
                let captured = capture
                    .node
                    .utf8_text(source.as_bytes())
                    .map_err(|err| format!("invalid utf8 in capture: {err}"))?;

                match name {
                    "type" if type_name_matches(captured, type_name) => matched_type = true,
                    "site" => site_node = Some(capture.node),
                    _ => {}
                }
            }

            if matched_type {
                if let Some(node) = site_node {
                    push_range(&mut ranges, node);
                }
            }
        }

        ranges.sort_by_key(|range| range.start);
        Ok(ranges)
    }
}

fn type_name_matches(captured: &str, type_name: &str) -> bool {
    captured == type_name || captured.ends_with(&format!("::{type_name}"))
}

fn enclosing_call_range(mut node: Node) -> LineRange {
    while let Some(parent) = node.parent() {
        if matches!(
            parent.kind(),
            "call_expression" | "method_invocation" | "call" | "function_call_expression"
        ) {
            node = parent;
            break;
        }
        node = parent;
    }
    LineRange {
        start: node.start_position().row + 1,
        end: node.end_position().row + 1,
    }
}

fn push_range(ranges: &mut Vec<LineRange>, node: Node) {
    let range = LineRange {
        start: node.start_position().row + 1,
        end: node.end_position().row + 1,
    };
    if !ranges.contains(&range) {
        ranges.push(range);
    }
}

fn call_grammar_for_language(language: SourceLanguage) -> Result<(Language, &'static str), String> {
    match language {
        SourceLanguage::Rust => Ok((tree_sitter_rust::LANGUAGE.into(), RUST_CALL_QUERY)),
        SourceLanguage::TypeScript => Ok((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TYPESCRIPT_CALL_QUERY,
        )),
        SourceLanguage::Tsx => Ok((
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            TYPESCRIPT_CALL_QUERY,
        )),
        SourceLanguage::Python => Ok((tree_sitter_python::LANGUAGE.into(), PYTHON_CALL_QUERY)),
        SourceLanguage::Java => Ok((tree_sitter_java::LANGUAGE.into(), JAVA_CALL_QUERY)),
        SourceLanguage::Kotlin => Ok((tree_sitter_kotlin_ng::LANGUAGE.into(), KOTLIN_CALL_QUERY)),
        SourceLanguage::Sql => Ok((tree_sitter_sequel::LANGUAGE.into(), SQL_CALL_QUERY)),
        SourceLanguage::C => Ok((tree_sitter_c::LANGUAGE.into(), C_CALL_QUERY)),
        SourceLanguage::Cpp => Ok((tree_sitter_cpp::LANGUAGE.into(), CPP_CALL_QUERY)),
        SourceLanguage::Unknown => Err("unsupported language for AST scan".to_string()),
    }
}

fn construction_grammar_for_language(
    language: SourceLanguage,
) -> Result<(Language, &'static str), String> {
    match language {
        SourceLanguage::Rust => Ok((tree_sitter_rust::LANGUAGE.into(), RUST_CONSTRUCTION_QUERY)),
        SourceLanguage::TypeScript => Ok((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TYPESCRIPT_CONSTRUCTION_QUERY,
        )),
        SourceLanguage::Tsx => Ok((
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            TYPESCRIPT_CONSTRUCTION_QUERY,
        )),
        SourceLanguage::Java => Ok((tree_sitter_java::LANGUAGE.into(), JAVA_CONSTRUCTION_QUERY)),
        SourceLanguage::Kotlin => Ok((
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            KOTLIN_CONSTRUCTION_QUERY,
        )),
        SourceLanguage::C => Ok((tree_sitter_c::LANGUAGE.into(), C_CONSTRUCTION_QUERY)),
        SourceLanguage::Cpp => Ok((tree_sitter_cpp::LANGUAGE.into(), CPP_CONSTRUCTION_QUERY)),
        SourceLanguage::Python | SourceLanguage::Sql | SourceLanguage::Unknown => {
            Err("construction grammar delegated or unsupported".to_string())
        }
    }
}

const RUST_CALL_QUERY: &str = r#"
(call_expression
  function: (identifier) @name)

(call_expression
  function: (field_expression
    field: (field_identifier) @name))

(call_expression
  function: (scoped_identifier
    name: (identifier) @name))
"#;

const RUST_CONSTRUCTION_QUERY: &str = r#"
(struct_expression
  name: (type_identifier) @type) @site

(struct_expression
  name: (scoped_type_identifier
    name: (type_identifier) @type)) @site
"#;

const TYPESCRIPT_CALL_QUERY: &str = r#"
(call_expression
  function: (identifier) @name)

(call_expression
  function: (member_expression
    property: (property_identifier) @name))
"#;

const TYPESCRIPT_CONSTRUCTION_QUERY: &str = r#"
(new_expression
  constructor: (identifier) @type) @site
"#;

const PYTHON_CALL_QUERY: &str = r#"
(call
  function: (identifier) @name)

(call
  function: (attribute
    attribute: (identifier) @name))
"#;

const JAVA_CALL_QUERY: &str = r#"
(method_invocation
  name: (identifier) @name)
"#;

const JAVA_CONSTRUCTION_QUERY: &str = r#"
(object_creation_expression
  type: (type_identifier) @type) @site
"#;

const KOTLIN_CALL_QUERY: &str = r#"
(call_expression
  (identifier) @name)

(call_expression
  (navigation_expression
    (identifier) @name))
"#;

const KOTLIN_CONSTRUCTION_QUERY: &str = r#"
(call_expression
  (simple_identifier) @type) @site
"#;

const SQL_CALL_QUERY: &str = r#"
(invocation
  (object_reference
    name: (identifier) @name))

(invocation
  (object_reference
    (identifier) @name))
"#;

const C_CALL_QUERY: &str = r#"
(call_expression
  function: (identifier) @name)

(call_expression
  function: (field_expression
    field: (field_identifier) @name))
"#;

const C_CONSTRUCTION_QUERY: &str = r#"
(call_expression
  function: (identifier) @type) @site
"#;

const CPP_CALL_QUERY: &str = r#"
(call_expression
  function: (identifier) @name)

(call_expression
  function: (field_expression
    field: (field_identifier) @name))

(call_expression
  function: (qualified_identifier
    name: (identifier) @name))
"#;

const CPP_CONSTRUCTION_QUERY: &str = r#"
(call_expression
  function: (identifier) @type) @site

(call_expression
  function: (qualified_identifier
    name: (identifier) @type)) @site
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/scout")
            .join(name)
    }

    #[test]
    fn ignores_comments_and_string_literals() {
        let lines =
            AstUsageFinder::find_calls_in_file(&fixture("sample.rs"), "invoke").expect("scan");

        assert_eq!(lines, vec![3, 5]);
    }

    #[test]
    fn finds_rust_struct_literal_ranges() {
        let ranges = AstUsageFinder::find_construction_sites_in_file(
            &fixture("struct_literal.rs"),
            "LogEvent",
        )
        .expect("scan");

        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start, 4);
        assert!(ranges[0].end >= ranges[0].start);
        assert_eq!(ranges[1].start, 19);
    }

    #[test]
    fn finds_java_method_invocation() {
        let path = fixture("sample.java");
        let lines = AstUsageFinder::find_calls_in_file(&path, "logSortEvent").expect("scan");
        assert_eq!(lines, vec![5]);
    }

    #[test]
    fn rejects_unknown_extension_without_detection() {
        let path = fixture("readme.txt");
        let err = AstUsageFinder::find_calls_in_file(&path, "alpha")
            .expect_err("text files should be rejected");

        assert!(
            err.contains("cannot determine language"),
            "unexpected: {err}"
        );
    }
}
