//! JavaScript symbol extraction.
//!
//! Covers class declarations with their methods (incl. getters,
//! setters and static members), free functions (incl. exported,
//! async and generator variants), function-valued bindings (arrow,
//! function and generator expressions) and top-level literal
//! constants. JSX needs no special casing: components are plain
//! functions or classes and the grammar parses JSX natively.

use anyhow::Result;
use tree_sitter::Node;

use super::{Kind, Symbol, line_of, node_text, parse};

/// Extract all symbols from a JavaScript source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = tree_sitter_javascript::LANGUAGE.into();
    let tree = parse(&language, source)?;
    let mut symbols = Vec::new();
    walk(tree.root_node(), source, None, true, &mut symbols);
    Ok(symbols)
}

/// Record the symbols of a node and descend into its children.
///
/// The scope carries the enclosing class name while inside a class
/// body. The top flag stays true only above any body, so literal
/// constants and function-valued bindings are limited to top-level
/// declarations.
///
/// @param node the syntax node to visit
/// @param source the raw file contents
/// @param scope the enclosing class name, or +nil+ outside classes
/// @param top whether the node still sits at the top level
/// @param symbols the symbol list collected so far
fn walk(
    node: Node<'_>,
    source: &[u8],
    scope: Option<&str>,
    top: bool,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "class_declaration" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            symbols.push(symbol(Kind::Class, node, name, source, scope));
            if let Some(body) = node.child_by_field_name("body") {
                let class_name = node_text(name, source);
                walk_children(body, source, Some(&class_name), false, symbols);
            }
        }
        "method_definition" => {
            if let Some(name) = node.child_by_field_name("name")
                && scope.is_some()
                && in_class(node)
            {
                symbols.push(symbol(Kind::Method, node, name, source, scope));
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, None, false, symbols);
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(symbol(Kind::Func, node, name, source, scope));
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, None, false, symbols);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            bindings(node, source, scope, top, symbols);
        }
        _ => {
            // Only the program root and export wrappers keep the
            // top-level status for their children
            let keep = matches!(node.kind(), "program" | "export_statement");
            walk_children(node, source, scope, top && keep, symbols);
        }
    }
}

/// Walk all named children of a node.
///
/// @param node the syntax node whose children to visit
/// @param source the raw file contents
/// @param scope the enclosing class name, or +nil+ outside classes
/// @param top whether the children still sit at the top level
/// @param symbols the symbol list collected so far
fn walk_children(
    node: Node<'_>,
    source: &[u8],
    scope: Option<&str>,
    top: bool,
    symbols: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, scope, top, symbols);
    }
}

/// Record the symbols of one variable declaration statement.
///
/// Function-valued bindings (arrow functions, function and generator
/// expressions) become functions; literal-valued +const+ bindings
/// become constants. Both are limited to top-level declarations.
///
/// @param node the declaration statement node
/// @param source the raw file contents
/// @param scope the enclosing scope, when any
/// @param top whether the declaration sits at the top level
/// @param symbols the symbol list collected so far
fn bindings(
    node: Node<'_>,
    source: &[u8],
    scope: Option<&str>,
    top: bool,
    symbols: &mut Vec<Symbol>,
) {
    let constant = node
        .child_by_field_name("kind")
        .is_some_and(|kind| kind.kind() == "const");
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" || !top {
            continue;
        }
        let Some(name) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };
        if name.kind() != "identifier" {
            continue;
        }
        if function_value(value.kind()) {
            symbols.push(symbol(Kind::Func, child, name, source, scope));
        } else if constant && literal_value(value.kind()) {
            symbols.push(symbol(Kind::Const, child, name, source, scope));
        }
    }
}

/// Whether the node is a direct member of a class body.
///
/// @param node the syntax node
/// @return whether the parent is a class body
fn in_class(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "class_body")
}

/// Whether the declarator value is a function-like expression.
///
/// @param kind the value node kind
/// @return whether the binding names a function
fn function_value(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "generator_function"
    )
}

/// Whether the declarator value is a literal.
///
/// @param kind the value node kind
/// @return whether the binding names a plain constant
fn literal_value(kind: &str) -> bool {
    matches!(
        kind,
        "string"
            | "template_string"
            | "number"
            | "true"
            | "false"
            | "null"
            | "undefined"
            | "object"
            | "array"
            | "regex"
    )
}

/// Build a symbol from a definition node and its name node.
///
/// @param kind the symbol kind
/// @param definition the definition node (carries the line)
/// @param name the name node
/// @param source the raw file contents
/// @param scope the enclosing scope, when any
/// @return the symbol
fn symbol(
    kind: Kind,
    definition: Node<'_>,
    name: Node<'_>,
    source: &[u8],
    scope: Option<&str>,
) -> Symbol {
    Symbol {
        line: line_of(definition),
        kind,
        name: node_text(name, source),
        scope: scope.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded JSX fixture exercised by the extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/javascript/sample.jsx");

    /// Map extracted symbols to comparable tuples.
    ///
    /// @param symbols the extracted symbols
    /// @return the (line, kind, name, scope) tuples
    fn tuples(symbols: &[Symbol]) -> Vec<(u32, Kind, &str, Option<&str>)> {
        symbols
            .iter()
            .map(|sym| {
                (sym.line, sym.kind, sym.name.as_str(), sym.scope.as_deref())
            })
            .collect()
    }

    #[test]
    fn extracts_the_expected_javascript_symbols() {
        let symbols = extract(FIXTURE).unwrap();
        assert_eq!(
            tuples(&symbols),
            vec![
                (3, Kind::Class, "Widget", None),
                (4, Kind::Method, "constructor", Some("Widget")),
                (8, Kind::Method, "title", Some("Widget")),
                (12, Kind::Method, "title", Some("Widget")),
                (16, Kind::Method, "create", Some("Widget")),
                (20, Kind::Method, "render", Some("Widget")),
                (25, Kind::Func, "Header", None),
                (27, Kind::Func, "legacy", None),
                (31, Kind::Func, "fetchWidgets", None),
                (36, Kind::Func, "widgetIds", None),
                (40, Kind::Const, "VERSION", None),
                (42, Kind::Const, "DEFAULTS", None),
            ]
        );
    }
}
