//! Erlang symbol extraction.
//!
//! Captures the module attribute, records, macro definitions, type
//! aliases and functions. Multi-clause functions collapse into one
//! symbol per name/arity pair. Erlang is flat, so no symbol carries a
//! scope.

use std::collections::HashSet;

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded definition query.
const QUERY: &str = include_str!("queries/erlang.scm");

/// Extract all symbols from an Erlang source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_erlang::LANGUAGE);
    // Track name/arity pairs so multi-clause functions collapse into
    // one symbol at their first clause
    let mut seen: HashSet<(String, usize)> = HashSet::new();
    run_query(&language, QUERY, source, |capture, node| match capture {
        "module" => Some(plain_symbol(node, source, Kind::Module)),
        "class" | "type" => Some(plain_symbol(node, source, Kind::Class)),
        "const" => macro_symbol(node, source),
        "func" => function_symbol(node, source, &mut seen),
        _ => None,
    })
}

/// Build a scope-less symbol from a name node.
///
/// @param node the name node
/// @param source the raw file contents
/// @param kind the symbol kind
/// @return the symbol
fn plain_symbol(node: Node<'_>, source: &[u8], kind: Kind) -> Symbol {
    Symbol {
        line: line_of(node),
        kind,
        name: node_text(node, source),
        scope: None,
    }
}

/// Build the symbol of a macro definition.
///
/// @param node the macro left-hand side node
/// @param source the raw file contents
/// @return the symbol, or +nil+ when the macro carries no name
fn macro_symbol(node: Node<'_>, source: &[u8]) -> Option<Symbol> {
    let name = node.child_by_field_name("name")?;
    Some(Symbol {
        line: line_of(node),
        kind: Kind::Const,
        name: node_text(name, source),
        scope: None,
    })
}

/// Build the symbol of a function declaration.
///
/// The grammar already groups adjacent clauses into one declaration;
/// the name/arity ledger additionally guards against stray duplicate
/// declarations. The symbol takes the first clause's line and its name
/// without the arity suffix.
///
/// @param node the function declaration node
/// @param source the raw file contents
/// @param seen the ledger of already recorded name/arity pairs
/// @return the symbol, or +nil+ for duplicates and nameless clauses
fn function_symbol(
    node: Node<'_>,
    source: &[u8],
    seen: &mut HashSet<(String, usize)>,
) -> Option<Symbol> {
    // Pick the first proper clause as the representative of the
    // whole function
    let mut cursor = node.walk();
    let clause = node
        .children_by_field_name("clause", &mut cursor)
        .find(|clause| clause.kind() == "function_clause")?;

    let name = node_text(clause.child_by_field_name("name")?, source);
    let arity = clause
        .child_by_field_name("args")
        .map_or(0, |args| args.named_child_count());
    seen.insert((name.clone(), arity)).then(|| Symbol {
        line: line_of(clause),
        kind: Kind::Func,
        name,
        scope: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded Erlang fixture.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/erlang/sample.erl");

    /// Extract all symbols from the fixture.
    ///
    /// @return the extracted symbols
    fn symbols() -> Vec<Symbol> {
        extract(FIXTURE).expect("extraction failed")
    }

    /// Fetch the first fixture symbol with the given name.
    ///
    /// @param name the symbol name to look up
    /// @return the matching symbol
    fn find(name: &str) -> Symbol {
        symbols()
            .into_iter()
            .find(|symbol| symbol.name == name)
            .expect("symbol not found")
    }

    /// Build a symbol literal for comparison.
    ///
    /// @param line the 1-based line
    /// @param kind the symbol kind
    /// @param name the display name
    /// @return the symbol
    fn sym(line: u32, kind: Kind, name: &str) -> Symbol {
        Symbol {
            line,
            kind,
            name: name.into(),
            scope: None,
        }
    }

    #[test]
    fn extracts_the_full_symbol_table() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, Kind::Module, "sample"),
                sym(4, Kind::Class, "point"),
                sym(6, Kind::Const, "PI"),
                sym(7, Kind::Const, "SQUARE"),
                sym(9, Kind::Class, "shape"),
                sym(11, Kind::Func, "area"),
                sym(14, Kind::Func, "area"),
            ]
        );
    }

    #[test]
    fn dedupes_multi_clause_functions_by_name_and_arity() {
        let count = symbols()
            .into_iter()
            .filter(|symbol| symbol.name == "area")
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn extracts_records_as_classes() {
        assert_eq!(find("point"), sym(4, Kind::Class, "point"));
    }

    #[test]
    fn extracts_defines_as_constants() {
        assert_eq!(find("PI"), sym(6, Kind::Const, "PI"));
    }

    #[test]
    fn extracts_types_as_classes() {
        assert_eq!(find("shape"), sym(9, Kind::Class, "shape"));
    }
}
