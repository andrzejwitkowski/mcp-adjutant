use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use super::lang::{detect_file_language, SourceLanguage};

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

        let (language, query_source) = grammar_for_language(report.language)?;

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
}

fn grammar_for_language(language: SourceLanguage) -> Result<(Language, &'static str), String> {
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

const TYPESCRIPT_CALL_QUERY: &str = r#"
(call_expression
  function: (identifier) @name)

(call_expression
  function: (member_expression
    property: (property_identifier) @name))
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

const KOTLIN_CALL_QUERY: &str = r#"
(call_expression
  (identifier) @name)

(call_expression
  (navigation_expression
    (identifier) @name))
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
