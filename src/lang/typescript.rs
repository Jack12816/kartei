//! TypeScript symbol extraction (both dialects).
//!
//! Builds on the JavaScript rules (classes with methods, free
//! functions, function-valued bindings, top-level literal constants)
//! and adds the TypeScript-only declarations: interfaces, type
//! aliases and enums (all recorded as class-like types), namespaces
//! (recorded as modules, with their members scoped by the dotted
//! namespace chain) and ambient function declarations. The TSX
//! dialect shares the exact same extraction logic and only swaps
//! the grammar.

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, parse};

/// Extract all symbols from a TypeScript source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    extract_with(&language, source)
}

/// Extract all symbols from a TSX source file.
///
/// Works the same way as +extract+ but parses with the TSX grammar.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract_tsx(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = tree_sitter_typescript::LANGUAGE_TSX.into();
    extract_with(&language, source)
}

/// Extract all symbols with the given dialect grammar.
///
/// @param language the TypeScript or TSX grammar
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
fn extract_with(language: &Language, source: &[u8]) -> Result<Vec<Symbol>> {
    let tree = parse(language, source)?;
    let mut symbols = Vec::new();
    walk(tree.root_node(), source, None, true, &mut symbols);
    Ok(symbols)
}

/// Record the symbols of a node and descend into its children.
///
/// The scope carries either the dotted namespace chain or, while
/// inside a class body, the plain class name (methods are scoped by
/// their class only, never by the full namespace path). The top flag
/// stays true at the program level and inside namespace bodies, so
/// literal constants and function-valued bindings are limited to
/// those declaration contexts.
///
/// @param node the syntax node to visit
/// @param source the raw file contents
/// @param scope the enclosing scope, when any
/// @param top whether the node sits in a declaration context
/// @param symbols the symbol list collected so far
fn walk(
    node: Node<'_>,
    source: &[u8],
    scope: Option<&str>,
    top: bool,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            symbols.push(symbol(Kind::Class, node, name, source, scope));
            if let Some(body) = node.child_by_field_name("body") {
                let class_name = node_text(name, source);
                walk_children(body, source, Some(&class_name), false, symbols);
            }
        }
        "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(symbol(Kind::Class, node, name, source, scope));
            }
        }
        "internal_module" | "module" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            symbols.push(symbol(Kind::Module, node, name, source, scope));
            let Some(body) = node.child_by_field_name("body") else {
                return;
            };
            // Namespace members carry the dotted namespace chain
            let module_name = node_text(name, source);
            let chain = match scope {
                Some(outer) => format!("{outer}.{module_name}"),
                None => module_name,
            };
            walk_children(body, source, Some(&chain), true, symbols);
        }
        "method_definition"
        | "method_signature"
        | "abstract_method_signature" => {
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
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature" => {
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
            // Only the program root, export wrappers and ambient
            // wrappers keep the declaration context for children
            let keep = matches!(
                node.kind(),
                "program" | "export_statement" | "ambient_declaration"
            );
            walk_children(node, source, scope, top && keep, symbols);
        }
    }
}

/// Walk all named children of a node.
///
/// @param node the syntax node whose children to visit
/// @param source the raw file contents
/// @param scope the enclosing scope, when any
/// @param top whether the children sit in a declaration context
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
/// become constants. Both are limited to declaration contexts (the
/// program level and namespace bodies).
///
/// @param node the declaration statement node
/// @param source the raw file contents
/// @param scope the enclosing scope, when any
/// @param top whether the declaration sits in a declaration context
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

    /// The embedded TypeScript fixture for the extraction tests.
    const TS_FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/typescript/sample.ts");

    /// The embedded TSX fixture for the extraction tests.
    const TSX_FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/typescript/sample.tsx");

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
    fn extracts_the_expected_typescript_symbols() {
        let symbols = extract(TS_FIXTURE).unwrap();
        assert_eq!(
            tuples(&symbols),
            vec![
                (3, Kind::Class, "Widget", None),
                (8, Kind::Class, "WidgetMap", None),
                (10, Kind::Class, "Status", None),
                (15, Kind::Module, "Registry", None),
                (16, Kind::Const, "LIMIT", Some("Registry")),
                (18, Kind::Func, "register", Some("Registry")),
                (22, Kind::Module, "Cache", Some("Registry")),
                (23, Kind::Func, "clear", Some("Registry.Cache")),
                (27, Kind::Func, "inspect", None),
                (29, Kind::Class, "Repository", None),
                (30, Kind::Method, "find", Some("Repository")),
                (32, Kind::Method, "count", Some("Repository")),
                (37, Kind::Class, "WidgetService", None),
                (40, Kind::Method, "add", Some("WidgetService")),
                (44, Kind::Method, "size", Some("WidgetService")),
                (49, Kind::Const, "MAX_WIDGETS", None),
                (51, Kind::Func, "makeWidget", None),
            ]
        );
    }

    #[test]
    fn extracts_the_expected_tsx_symbols() {
        let symbols = extract_tsx(TSX_FIXTURE).unwrap();
        assert_eq!(
            tuples(&symbols),
            vec![
                (3, Kind::Class, "GreetingProps", None),
                (7, Kind::Func, "Greeting", None),
                (11, Kind::Func, "App", None),
            ]
        );
    }
}
