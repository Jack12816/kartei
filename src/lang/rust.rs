//! Rust symbol extraction.
//!
//! Uses a manual tree walk instead of a definition query because the
//! scope path (nested +mod+ items plus the +impl+ type for methods)
//! needs the full ancestor context which a flat query cannot carry.

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, parse};

/// Extract all symbols from a Rust source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_rust::LANGUAGE);
    let tree = parse(&language, source)?;
    let mut symbols = Vec::new();
    walk(
        tree.root_node(),
        source,
        &mut Vec::new(),
        false,
        &mut symbols,
    );
    Ok(symbols)
}

/// Record the node when it defines a symbol and recurse into its
/// children.
///
/// @param node the current syntax node
/// @param source the raw file contents
/// @param scope the enclosing scope segments (mod path, impl type)
/// @param in_impl whether the node sits directly inside an impl block
/// @param symbols the collected symbols
fn walk(
    node: Node<'_>,
    source: &[u8],
    scope: &mut Vec<String>,
    in_impl: bool,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "mod_item" => {
            let Some(name) = name_of(node, source) else {
                return;
            };
            symbols.push(symbol(node, Kind::Module, &name, scope));
            scope.push(name);
            walk_children(node, source, scope, false, symbols);
            scope.pop();
        }
        "impl_item" => {
            // The impl type extends the scope of its items, but the
            // impl block itself is no symbol
            let Some(name) = impl_type_of(node, source) else {
                return;
            };
            scope.push(name);
            walk_children(node, source, scope, true, symbols);
            scope.pop();
        }
        "function_item" => {
            let kind = match in_impl {
                true => Kind::Method,
                false => Kind::Func,
            };
            if let Some(name) = name_of(node, source) {
                symbols.push(symbol(node, kind, &name, scope));
            }
            walk_children(node, source, scope, false, symbols);
        }
        "struct_item" | "enum_item" | "union_item" | "trait_item"
        | "type_item" => {
            if let Some(name) = name_of(node, source) {
                symbols.push(symbol(node, Kind::Class, &name, scope));
            }
            walk_children(node, source, scope, false, symbols);
        }
        "const_item" | "static_item" => {
            if let Some(name) = name_of(node, source) {
                symbols.push(symbol(node, Kind::Const, &name, scope));
            }
        }
        "macro_definition" => {
            if let Some(name) = name_of(node, source) {
                symbols.push(symbol(node, Kind::Func, &name, scope));
            }
        }
        _ => walk_children(node, source, scope, in_impl, symbols),
    }
}

/// Walk all children of a node.
///
/// @param node the current syntax node
/// @param source the raw file contents
/// @param scope the enclosing scope segments (mod path, impl type)
/// @param in_impl whether the children sit directly inside an impl
///   block
/// @param symbols the collected symbols
fn walk_children(
    node: Node<'_>,
    source: &[u8],
    scope: &mut Vec<String>,
    in_impl: bool,
    symbols: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, scope, in_impl, symbols);
    }
}

/// Build a symbol for the given node.
///
/// @param node the defining syntax node
/// @param kind the symbol kind
/// @param name the display name
/// @param scope the enclosing scope segments
/// @return the symbol
fn symbol(node: Node<'_>, kind: Kind, name: &str, scope: &[String]) -> Symbol {
    let scope = match scope.is_empty() {
        true => None,
        false => Some(scope.join("::")),
    };
    Symbol {
        line: line_of(node),
        kind,
        name: name.into(),
        scope,
    }
}

/// Fetch the name field text of a node.
///
/// @param node the syntax node
/// @param source the raw file contents
/// @return the name text, or +nil+ for anonymous nodes
fn name_of(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .map(|name| node_text(name, source))
}

/// Fetch the base type name of an impl block.
///
/// Strips generic arguments so +impl Foo<T>+ scopes as +Foo+.
///
/// @param node the impl item node
/// @param source the raw file contents
/// @return the type name, or +nil+ for unnamed types
fn impl_type_of(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut type_node = node.child_by_field_name("type")?;
    if type_node.kind() == "generic_type" {
        type_node = type_node.child_by_field_name("type")?;
    }
    Some(node_text(type_node, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded Rust fixture.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/rust/sample.rs");

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
    /// @param scope the enclosing scope, when any
    /// @return the symbol
    fn sym(line: u32, kind: Kind, name: &str, scope: Option<&str>) -> Symbol {
        Symbol {
            line,
            kind,
            name: name.into(),
            scope: scope.map(String::from),
        }
    }

    #[test]
    fn extracts_the_full_symbol_table() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, Kind::Module, "outer", None),
                sym(2, Kind::Class, "Widget", Some("outer")),
                sym(7, Kind::Const, "SCALE", Some("outer::Widget")),
                sym(9, Kind::Method, "draw", Some("outer::Widget")),
                sym(12, Kind::Module, "inner", Some("outer")),
                sym(13, Kind::Const, "DEPTH", Some("outer::inner")),
                sym(17, Kind::Class, "Render", None),
                sym(21, Kind::Class, "Shade", None),
                sym(26, Kind::Class, "Raw", None),
                sym(31, Kind::Class, "Alias", None),
                sym(33, Kind::Const, "LIMIT", None),
                sym(35, Kind::Func, "widget", None),
                sym(39, Kind::Func, "free", None),
            ]
        );
    }

    #[test]
    fn scopes_impl_methods_by_their_type_name() {
        assert_eq!(
            find("draw"),
            sym(9, Kind::Method, "draw", Some("outer::Widget"))
        );
    }

    #[test]
    fn composes_nested_mod_scopes() {
        assert_eq!(
            find("DEPTH"),
            sym(13, Kind::Const, "DEPTH", Some("outer::inner"))
        );
    }

    #[test]
    fn extracts_traits_as_classes() {
        assert_eq!(find("Render"), sym(17, Kind::Class, "Render", None));
    }

    #[test]
    fn extracts_macro_rules_as_functions() {
        assert_eq!(find("widget"), sym(35, Kind::Func, "widget", None));
    }

    #[test]
    fn extracts_statics_as_constants() {
        assert_eq!(find("LIMIT"), sym(33, Kind::Const, "LIMIT", None));
    }
}
