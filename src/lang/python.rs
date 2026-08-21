//! Python symbol extraction.
//!
//! Covers classes (incl. nested ones, scoped by the enclosing class
//! chain), methods, module-level functions and ALL-CAPS module
//! constants. Decorated definitions are unwrapped by the walk, so
//! the recorded line is the +def+/+class+ line, not the decorator
//! line. Functions nested inside other functions are extracted as
//! functions with the enclosing definition chain as scope, so
//! closures and factory helpers stay findable.

use anyhow::Result;
use tree_sitter::Node;

use super::{Kind, Symbol, line_of, node_text, parse};

/// The immediate enclosing context during the tree walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// Module level, incl. compound statements at module level.
    Module,
    /// Directly inside a class body.
    Class,
    /// Inside a function body.
    Function,
}

/// Extract all symbols from a Python source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = tree_sitter_python::LANGUAGE.into();
    let tree = parse(&language, source)?;
    let mut symbols = Vec::new();
    walk(tree.root_node(), source, &[], Context::Module, &mut symbols);
    Ok(symbols)
}

/// Record the symbols of a node and descend into its children.
///
/// The chain carries the names of all enclosing class and function
/// definitions and is joined with dots to form the scope. The
/// context decides whether a definition is a method (directly
/// inside a class body) and whether assignments count as module
/// constants.
///
/// @param node the syntax node to visit
/// @param source the raw file contents
/// @param chain the enclosing definition names collected so far
/// @param context the immediate enclosing context
/// @param symbols the symbol list collected so far
fn walk(
    node: Node<'_>,
    source: &[u8],
    chain: &[String],
    context: Context,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "class_definition" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            symbols.push(symbol(Kind::Class, node, name, source, chain));
            descend(node, name, source, chain, Context::Class, symbols);
        }
        "function_definition" => {
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            // Only definitions directly inside a class body are
            // methods; everything else is a plain function
            let kind = match context {
                Context::Class => Kind::Method,
                _ => Kind::Func,
            };
            symbols.push(symbol(kind, node, name, source, chain));
            descend(node, name, source, chain, Context::Function, symbols);
        }
        "assignment" if context == Context::Module => {
            constant(node, source, symbols);
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk(child, source, chain, context, symbols);
            }
        }
    }
}

/// Walk the body of a definition with its name added to the chain.
///
/// @param node the definition node
/// @param name the definition name node
/// @param source the raw file contents
/// @param chain the enclosing definition names collected so far
/// @param context the context the body opens
/// @param symbols the symbol list collected so far
fn descend(
    node: Node<'_>,
    name: Node<'_>,
    source: &[u8],
    chain: &[String],
    context: Context,
    symbols: &mut Vec<Symbol>,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut inner = chain.to_vec();
    inner.push(node_text(name, source));
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        walk(child, source, &inner, context, symbols);
    }
}

/// Record a module-level ALL-CAPS assignment as a constant.
///
/// @param node the assignment node
/// @param source the raw file contents
/// @param symbols the symbol list collected so far
fn constant(node: Node<'_>, source: &[u8], symbols: &mut Vec<Symbol>) {
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    // Annotation-only statements carry no right-hand side and thus
    // define no value to record
    if left.kind() != "identifier"
        || node.child_by_field_name("right").is_none()
    {
        return;
    }
    let name = node_text(left, source);
    if !constant_name(&name) {
        return;
    }
    symbols.push(Symbol {
        line: line_of(node),
        kind: Kind::Const,
        name,
        scope: None,
    });
}

/// Whether the name follows the ALL-CAPS constant convention.
///
/// Underscores and digits are allowed, but at least one uppercase
/// letter is required.
///
/// @param name the assignment target name
/// @return whether the name reads as a constant
fn constant_name(name: &str) -> bool {
    let allowed = name.chars().all(|chr| {
        chr.is_ascii_uppercase() || chr.is_ascii_digit() || chr == '_'
    });
    allowed && name.chars().any(|chr| chr.is_ascii_uppercase())
}

/// Build a symbol from a definition node and its name node.
///
/// @param kind the symbol kind
/// @param definition the definition node (carries the line)
/// @param name the name node
/// @param source the raw file contents
/// @param chain the enclosing definition names
/// @return the symbol
fn symbol(
    kind: Kind,
    definition: Node<'_>,
    name: Node<'_>,
    source: &[u8],
    chain: &[String],
) -> Symbol {
    Symbol {
        line: line_of(definition),
        kind,
        name: node_text(name, source),
        scope: (!chain.is_empty()).then(|| chain.join(".")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded Python fixture for the extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/python/sample.py");

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
    fn extracts_the_expected_python_symbols() {
        let symbols = extract(FIXTURE).unwrap();
        assert_eq!(
            tuples(&symbols),
            vec![
                (5, Kind::Const, "VERSION", None),
                (6, Kind::Const, "MAX_RETRIES", None),
                (10, Kind::Func, "plain", None),
                (15, Kind::Func, "fetch", None),
                (21, Kind::Func, "memoized", None),
                (26, Kind::Func, "outer", None),
                (29, Kind::Func, "inner", Some("outer")),
                (35, Kind::Class, "Widget", None),
                (39, Kind::Method, "title", Some("Widget")),
                (42, Kind::Class, "Meta", Some("Widget")),
                (45, Kind::Method, "describe", Some("Widget.Meta")),
                (49, Kind::Class, "Registry", None),
                (52, Kind::Method, "register", Some("Registry")),
            ]
        );
    }
}
