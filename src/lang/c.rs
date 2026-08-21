//! C symbol extraction.
//!
//! Captures function definitions (never prototypes), tagged type
//! definitions, typedef names and preprocessor macros. C symbols carry
//! no scope; every extracted symbol is top level.

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded definition query.
const QUERY: &str = include_str!("queries/c.scm");

/// Extract all symbols from a C source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_c::LANGUAGE);
    // Extra names of multi-name typedefs (`typedef int a, b;`) which
    // the one-symbol-per-capture builder cannot return inline
    let mut extra: Vec<Symbol> = Vec::new();
    let mut symbols =
        run_query(&language, QUERY, source, |capture, node| match capture {
            "func" => function_symbol(node, source),
            "class" => Some(plain_symbol(node, source, Kind::Class)),
            "const" => Some(plain_symbol(node, source, Kind::Const)),
            "typedef" => typedef_symbol(node, source, &mut extra),
            _ => None,
        })?;
    symbols.append(&mut extra);
    symbols.sort_by_key(|symbol| symbol.line);
    Ok(symbols)
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

/// Build the symbol of a function definition.
///
/// @param node the function definition node
/// @param source the raw file contents
/// @return the symbol, or +nil+ when no defining identifier exists
fn function_symbol(node: Node<'_>, source: &[u8]) -> Option<Symbol> {
    let declarator = node.child_by_field_name("declarator")?;
    let inner = innermost_declarator(declarator);
    (inner.kind() == "identifier").then(|| Symbol {
        line: line_of(node),
        kind: Kind::Func,
        name: node_text(inner, source),
        scope: None,
    })
}

/// Build the symbols of a typedef statement.
///
/// The first declared name is returned, any further names of the same
/// statement are pushed onto the overflow list.
///
/// @param node the type definition node
/// @param source the raw file contents
/// @param extra the overflow list for additional declared names
/// @return the first symbol, or +nil+ when no name resolves
fn typedef_symbol(
    node: Node<'_>,
    source: &[u8],
    extra: &mut Vec<Symbol>,
) -> Option<Symbol> {
    let mut cursor = node.walk();
    let mut names: Vec<String> = Vec::new();
    for declarator in node.children_by_field_name("declarator", &mut cursor) {
        let inner = innermost_declarator(declarator);
        if inner.kind() == "type_identifier" {
            names.push(node_text(inner, source));
        }
    }

    let mut names = names.into_iter();
    let first = names.next()?;
    for name in names {
        extra.push(Symbol {
            line: line_of(node),
            kind: Kind::Class,
            name,
            scope: None,
        });
    }
    Some(Symbol {
        line: line_of(node),
        kind: Kind::Class,
        name: first,
        scope: None,
    })
}

/// Descend a declarator chain to its terminal name node.
///
/// Peels pointer, array, function and parenthesized declarators so
/// even function pointer chains resolve to the inner name.
///
/// @param node the outermost declarator node
/// @return the terminal declarator node
fn innermost_declarator(node: Node<'_>) -> Node<'_> {
    let mut current = node;
    loop {
        let next = current.child_by_field_name("declarator").or_else(|| {
            match current.kind() == "parenthesized_declarator" {
                true => current.named_child(0),
                false => None,
            }
        });
        match next {
            Some(inner) => current = inner,
            None => return current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded C fixture.
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/c/sample.c");

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
                sym(3, Kind::Const, "MAX_ITEMS"),
                sym(4, Kind::Const, "SQUARE"),
                sym(6, Kind::Class, "point"),
                sym(11, Kind::Class, "value"),
                sym(16, Kind::Class, "color"),
                sym(21, Kind::Class, "point_t"),
                sym(23, Kind::Class, "dims_t"),
                sym(30, Kind::Func, "add"),
                sym(35, Kind::Func, "greeting"),
            ]
        );
    }

    #[test]
    fn skips_function_prototypes() {
        let count = symbols()
            .into_iter()
            .filter(|symbol| symbol.name == "add")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extracts_typedef_names_as_classes() {
        assert_eq!(find("dims_t"), sym(23, Kind::Class, "dims_t"));
    }

    #[test]
    fn extracts_function_like_macros_as_constants() {
        assert_eq!(find("SQUARE"), sym(4, Kind::Const, "SQUARE"));
    }

    #[test]
    fn extracts_pointer_returning_functions() {
        assert_eq!(find("greeting"), sym(35, Kind::Func, "greeting"));
    }
}
