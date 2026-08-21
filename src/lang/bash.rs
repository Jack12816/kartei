//! Bash symbol extraction.
//!
//! Extraction runs the embedded definition query for function
//! definitions (the `function foo()` and the `foo()` form parse to
//! the same node) and variable assignments. Assignments inside a
//! function body are dropped since they are runtime-local state, not
//! script-level definitions. Bash has no nesting construct we track,
//! so every symbol is scope-free.

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded Bash definition query.
const QUERY: &str = include_str!("queries/bash.scm");

/// Extract all symbols from a Bash source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_bash::LANGUAGE);
    run_query(&language, QUERY, source, |capture, node| {
        let kind = match capture {
            "definition.func" => Kind::Func,
            "definition.var" if !inside_function(node) => Kind::Env,
            _ => return None,
        };
        Some(Symbol {
            line: line_of(node),
            kind,
            name: node_text(node, source),
            scope: None,
        })
    })
}

/// Whether the node lies inside a function body.
///
/// @param node the captured name node
/// @return whether a function definition encloses the node
fn inside_function(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(ancestor) = current.parent() {
        if ancestor.kind() == "function_definition" {
            return true;
        }
        current = ancestor;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Bash fixture exercised by all extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/bash/sample.sh");

    /// Extract all symbols from the fixture.
    ///
    /// @return the extracted symbols
    fn symbols() -> Vec<Symbol> {
        extract(FIXTURE).unwrap()
    }

    /// Build a symbol literal for the assertions.
    ///
    /// @param line the 1-based line
    /// @param kind the symbol kind
    /// @param name the display name
    /// @return the scope-free symbol
    fn sym(line: u32, kind: Kind, name: &str) -> Symbol {
        Symbol {
            line,
            kind,
            name: name.to_string(),
            scope: None,
        }
    }

    #[test]
    fn extracts_top_level_variables() {
        assert!(symbols().contains(&sym(3, Kind::Env, "PROGNAME")));
    }

    #[test]
    fn extracts_keyword_form_functions() {
        assert!(symbols().contains(&sym(6, Kind::Func, "first-helper")));
    }

    #[test]
    fn extracts_parens_form_functions() {
        assert!(symbols().contains(&sym(12, Kind::Func, "second_helper")));
    }

    #[test]
    fn extracts_declared_top_level_variables() {
        assert!(symbols().contains(&sym(16, Kind::Env, "LIMIT")));
    }

    #[test]
    fn skips_variables_inside_functions() {
        assert!(!symbols().iter().any(|found| found.name == "INNER_TOO"));
    }

    #[test]
    fn skips_local_variables_inside_functions() {
        assert!(!symbols().iter().any(|found| found.name == "inner"));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(3, Kind::Env, "PROGNAME"),
                sym(4, Kind::Env, "COLOR"),
                sym(6, Kind::Func, "first-helper"),
                sym(12, Kind::Func, "second_helper"),
                sym(16, Kind::Env, "LIMIT"),
            ]
        );
    }
}
