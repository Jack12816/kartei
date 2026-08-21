//! Markdown symbol extraction.
//!
//! Extraction runs the embedded definition query for ATX (`#` up to
//! `######`) and setext (underlined) headings against the block
//! grammar only; the inline grammar adds nothing for symbol names.
//! The recorded name is the heading text without the marker, so
//! `## Usage` and an underlined `Usage` read the same. Headings carry
//! no scope.

use anyhow::Result;
use tree_sitter::Language;

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded Markdown definition query.
const QUERY: &str = include_str!("queries/markdown.scm");

/// Extract all symbols from a Markdown document.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_md::LANGUAGE);
    run_query(&language, QUERY, source, |_, node| {
        // The content field skips the ATX marker and, for setext
        // headings, the underline; markerless headings are dropped
        let content = node.child_by_field_name("heading_content")?;
        let name = node_text(content, source).trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Symbol {
            line: line_of(node),
            kind: Kind::Heading,
            name,
            scope: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Markdown fixture exercised by all extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/markdown/sample.md");

    /// Extract all symbols from the fixture.
    ///
    /// @return the extracted symbols
    fn symbols() -> Vec<Symbol> {
        extract(FIXTURE).unwrap()
    }

    /// Build a symbol literal for the assertions.
    ///
    /// @param line the 1-based line
    /// @param name the heading text
    /// @return the scope-free heading symbol
    fn sym(line: u32, name: &str) -> Symbol {
        Symbol {
            line,
            kind: Kind::Heading,
            name: name.to_string(),
            scope: None,
        }
    }

    #[test]
    fn extracts_atx_headings_without_the_marker() {
        assert!(symbols().contains(&sym(1, "Title")));
    }

    #[test]
    fn extracts_setext_headings_without_the_underline() {
        assert!(symbols().contains(&sym(7, "Setext Top")));
    }

    #[test]
    fn skips_hashes_inside_fenced_code_blocks() {
        assert!(!symbols().iter().any(|found| found.name.contains("not")));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, "Title"),
                sym(5, "Section One"),
                sym(7, "Setext Top"),
                sym(10, "Setext Sub"),
                sym(17, "Details"),
                sym(19, "Fine Print"),
            ]
        );
    }
}
