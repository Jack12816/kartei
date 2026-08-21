//! Make symbol extraction.
//!
//! Extraction runs the embedded definition query for rule targets and
//! variable assignments. A rule with several targets contributes one
//! symbol per target. GNU make special targets (`.PHONY`, `.SUFFIXES`,
//! ...) and pattern targets (`%.o`) are dropped since they name no
//! addressable build step; dot-prefixed helper targets in lowercase
//! (eg. `.env-check`) stay in. Make has no scoping, so every symbol is
//! scope-free.

use anyhow::Result;
use tree_sitter::Language;

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded Make definition query.
const QUERY: &str = include_str!("queries/make.scm");

/// Extract all symbols from a Make source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_make::LANGUAGE);
    run_query(&language, QUERY, source, |capture, node| {
        let name = node_text(node, source);
        let kind = match capture {
            "definition.target" if !skippable_target(&name) => Kind::Target,
            "definition.var" => Kind::Env,
            _ => return None,
        };
        Some(Symbol {
            line: line_of(node),
            kind,
            name,
            scope: None,
        })
    })
}

/// Whether the target name is a special or pattern target.
///
/// GNU make special targets are dot-prefixed and all-uppercase
/// (`.PHONY`, `.DELETE_ON_ERROR`), which keeps dot-prefixed helper
/// targets like `.env-check` extractable.
///
/// @param name the raw target name
/// @return whether to drop the target
fn skippable_target(name: &str) -> bool {
    // Pattern targets carry no addressable name
    if name.contains('%') {
        return true;
    }
    name.strip_prefix('.').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|chr| chr.is_ascii_uppercase() || chr == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Make fixture exercised by all extraction tests.
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/make/Makefile");

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
    fn extracts_all_four_assignment_operators() {
        assert_eq!(
            symbols()
                .iter()
                .filter(|found| found.kind == Kind::Env)
                .count(),
            4
        );
    }

    #[test]
    fn extracts_every_target_of_a_multi_target_rule() {
        assert!(symbols().contains(&sym(10, Kind::Target, "test")));
    }

    #[test]
    fn skips_special_dot_targets() {
        assert!(!symbols().iter().any(|found| found.name == ".PHONY"));
    }

    #[test]
    fn skips_pattern_targets() {
        assert!(!symbols().iter().any(|found| found.name.contains('%')));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, Kind::Env, "NAME"),
                sym(2, Kind::Env, "PREFIX"),
                sym(3, Kind::Env, "CC"),
                sym(4, Kind::Env, "CFLAGS"),
                sym(8, Kind::Target, "all"),
                sym(10, Kind::Target, "build"),
                sym(10, Kind::Target, "test"),
                sym(16, Kind::Target, "clean"),
            ]
        );
    }
}
