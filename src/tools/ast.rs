use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

pub struct AstUsageFinder;

impl AstUsageFinder {
    pub fn find_calls_in_file(file_path: &Path, method_name: &str) -> Result<Vec<usize>, String> {
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .ok_or_else(|| format!("unsupported file type: {}", file_path.display()))?;

        let (language, query_source) = grammar_for_extension(extension)?;

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

fn grammar_for_extension(ext: &str) -> Result<(Language, &'static str), String> {
    match ext {
        "rs" => Ok((tree_sitter_rust::LANGUAGE.into(), RUST_CALL_QUERY)),
        "ts" => Ok((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TYPESCRIPT_CALL_QUERY,
        )),
        "tsx" => Ok((
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            TYPESCRIPT_CALL_QUERY,
        )),
        other => Err(format!("unsupported extension for AST scan: .{other}")),
    }
}

// ponytail: one query covers direct, method, and scoped calls — tree-sitter ignores comments/strings
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
    fn rejects_unknown_extension() {
        let path = fixture("readme.txt");
        let err = AstUsageFinder::find_calls_in_file(&path, "alpha")
            .expect_err("text files should be rejected");

        assert!(err.contains("unsupported"), "unexpected error: {err}");
    }
}
