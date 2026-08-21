//! Dockerfile symbol extraction.
//!
//! Extraction runs the embedded definition query for base images,
//! build-stage aliases, build arguments and environment variables.
//! Image references are kept exactly as written (tag or digest
//! included, eg. `ruby:3.4-slim`) so lookups match what the
//! Dockerfile states. RUN and CMD lines define no addressable name
//! and are deliberately not extracted. Dockerfiles have no scoping,
//! so every symbol is scope-free.
//!
//! The upstream grammar crate pins an incompatible tree-sitter core,
//! so the generated C sources are vendored and compiled by build.rs
//! (see vendor/tree-sitter-dockerfile); we bind its entry point here.

use anyhow::Result;
use tree_sitter::Language;
use tree_sitter_language::LanguageFn;

use super::{Kind, Symbol, line_of, node_text, run_query};

unsafe extern "C" {
    /// The entry point of the vendored Dockerfile grammar.
    fn tree_sitter_dockerfile() -> *const ();
}

/// The embedded Dockerfile definition query.
const QUERY: &str = include_str!("queries/dockerfile.scm");

/// Fetch the vendored Dockerfile grammar.
///
/// @return the loaded grammar
fn language() -> Language {
    // The entry point matches the tree-sitter language ABI, which is
    // exactly what from_raw requires
    Language::new(unsafe { LanguageFn::from_raw(tree_sitter_dockerfile) })
}

/// Extract all symbols from a Dockerfile.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    run_query(&language(), QUERY, source, |capture, node| {
        let kind = match capture {
            "definition.image" => Kind::Image,
            "definition.arg" => Kind::Arg,
            "definition.env" => Kind::Env,
            // Build-stage aliases carry no lookup value, skip them
            "definition.stage" => return None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Dockerfile fixture exercised by all extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/dockerfile/Dockerfile");

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
    fn keeps_the_image_reference_as_written() {
        assert!(symbols().contains(&sym(3, Kind::Image, "ruby:3.4-slim")));
    }

    #[test]
    fn skips_build_stage_aliases() {
        assert!(!symbols().iter().any(|found| found.name == "build"));
    }

    #[test]
    fn extracts_args_without_a_default() {
        assert!(symbols().contains(&sym(4, Kind::Arg, "BUNDLE_JOBS")));
    }

    #[test]
    fn splits_multi_pair_env_lines() {
        assert!(symbols().contains(&sym(5, Kind::Env, "BUNDLE_PATH")));
    }

    #[test]
    fn extracts_legacy_space_separated_env_lines() {
        assert!(symbols().contains(&sym(6, Kind::Env, "LEGACY_HOME")));
    }

    #[test]
    fn skips_run_and_cmd_lines() {
        assert!(!symbols().iter().any(|found| found.name.contains("puma")));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, Kind::Arg, "BASE_TAG"),
                sym(3, Kind::Image, "ruby:3.4-slim"),
                sym(4, Kind::Arg, "BUNDLE_JOBS"),
                sym(5, Kind::Env, "RAILS_ENV"),
                sym(5, Kind::Env, "BUNDLE_PATH"),
                sym(6, Kind::Env, "LEGACY_HOME"),
                sym(9, Kind::Image, "ruby:3.4-slim"),
                sym(10, Kind::Env, "PORT"),
            ]
        );
    }
}
