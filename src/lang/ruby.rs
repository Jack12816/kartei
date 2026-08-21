//! Ruby symbol extraction.
//!
//! Extraction runs the embedded definition query (an extended version
//! of the official tree-sitter-ruby tags query) and derives the
//! enclosing `Foo::Bar` scope per capture by walking the ancestor
//! chain. Compound declaration names (`class Foo::Bar`) keep their
//! written form as the symbol name, and members below them inherit
//! that written form in their scope (`Foo::Bar`). Methods defined
//! inside a `class << self` block are recorded as singleton methods
//! of the enclosing class, since that is what they define at runtime.

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Kind, Symbol, line_of, node_text, run_query};

/// The embedded Ruby definition query.
const QUERY: &str = include_str!("queries/ruby.scm");

/// Extract all symbols from a Ruby source file.
///
/// @param source the raw file contents
/// @return the extracted symbols
/// @raise when the grammar or query fails to load
pub fn extract(source: &[u8]) -> Result<Vec<Symbol>> {
    let language = Language::new(tree_sitter_ruby::LANGUAGE);
    run_query(&language, QUERY, source, |capture, node| {
        let kind = match capture {
            "definition.class" => Kind::Class,
            "definition.module" => Kind::Module,
            "definition.method" => method_kind(node),
            "definition.smethod" => Kind::Smethod,
            "definition.const" => Kind::Const,
            "definition.attr" => Kind::Method,
            _ => return None,
        };
        // Attr arguments are symbol literals which carry a leading
        // colon in the source; all other names never start with one
        let name = node_text(node, source).trim_start_matches(':').to_owned();
        Some(Symbol {
            line: line_of(node),
            kind,
            name,
            scope: scope_of(node, source),
        })
    })
}

/// Decide the kind of a plain `def` by its lexical position.
///
/// A method written inside a `class << self` block defines a
/// singleton method on the enclosing class, so we record it as such.
///
/// @param node the captured method name node
/// @return the singleton-method kind inside a singleton class, the
///   instance-method kind otherwise
fn method_kind(node: Node<'_>) -> Kind {
    let mut current = node;
    while let Some(ancestor) = current.parent() {
        match ancestor.kind() {
            "singleton_class" => return Kind::Smethod,
            "class" | "module" => return Kind::Method,
            _ => {}
        }
        current = ancestor;
    }
    Kind::Method
}

/// Build the enclosing module/class scope of a captured name node.
///
/// Walks the ancestor chain and joins the names of all enclosing
/// class and module declarations with `::`, outermost first. The
/// declaration a name belongs to is not part of its own scope, and
/// `class << self` blocks are transparent (their members scope to
/// the enclosing class).
///
/// @param node the captured name node
/// @param source the raw file contents
/// @return the scope, or +nil+ at top level
fn scope_of(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = node;
    while let Some(ancestor) = current.parent() {
        if matches!(ancestor.kind(), "class" | "module") {
            let name = ancestor.child_by_field_name("name");
            // Skip the declaration the captured node names itself
            if name.is_some_and(|found| found.id() != node.id()) {
                parts.push(node_text(name.unwrap(), source));
            }
        }
        current = ancestor;
    }

    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("::"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Ruby fixture exercised by all extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/ruby/sample.rb");

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
    /// @param scope the enclosing scope, when any
    /// @return the symbol
    fn sym(line: u32, kind: Kind, name: &str, scope: Option<&str>) -> Symbol {
        Symbol {
            line,
            kind,
            name: name.to_string(),
            scope: scope.map(str::to_string),
        }
    }

    #[test]
    fn extracts_top_level_constants() {
        assert!(symbols().contains(&sym(3, Kind::Const, "VERSION", None)));
    }

    #[test]
    fn extracts_scoped_constants() {
        assert!(symbols().contains(&sym(
            6,
            Kind::Const,
            "DEFAULT",
            Some("Foo")
        )));
    }

    #[test]
    fn extracts_top_level_modules() {
        assert!(symbols().contains(&sym(5, Kind::Module, "Foo", None)));
    }

    #[test]
    fn extracts_nested_classes_with_scope() {
        assert!(symbols().contains(&sym(8, Kind::Class, "Bar", Some("Foo"))));
    }

    #[test]
    fn extracts_attr_accessor_symbols_as_methods() {
        assert!(symbols().contains(&sym(
            9,
            Kind::Method,
            "first",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_every_attr_accessor_symbol() {
        assert!(symbols().contains(&sym(
            9,
            Kind::Method,
            "second",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_attr_reader_symbols() {
        assert!(symbols().contains(&sym(
            10,
            Kind::Method,
            "third",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_attr_writer_symbols() {
        assert!(symbols().contains(&sym(
            11,
            Kind::Method,
            "fourth",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_instance_methods_with_nested_scope() {
        assert!(symbols().contains(&sym(
            13,
            Kind::Method,
            "initialize",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_singleton_methods() {
        assert!(symbols().contains(&sym(
            17,
            Kind::Smethod,
            "build",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_singleton_class_methods_as_singleton_methods() {
        assert!(symbols().contains(&sym(
            22,
            Kind::Smethod,
            "cached",
            Some("Foo::Bar")
        )));
    }

    #[test]
    fn extracts_singleton_methods_in_nested_modules() {
        assert!(symbols().contains(&sym(
            29,
            Kind::Smethod,
            "helper",
            Some("Foo::Util")
        )));
    }

    #[test]
    fn extracts_compound_class_names_verbatim() {
        assert!(symbols().contains(&sym(33, Kind::Class, "Foo::Baz", None)));
    }

    #[test]
    fn extracts_methods_below_compound_class_names() {
        assert!(symbols().contains(&sym(
            34,
            Kind::Method,
            "qux",
            Some("Foo::Baz")
        )));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(3, Kind::Const, "VERSION", None),
                sym(5, Kind::Module, "Foo", None),
                sym(6, Kind::Const, "DEFAULT", Some("Foo")),
                sym(8, Kind::Class, "Bar", Some("Foo")),
                sym(9, Kind::Method, "first", Some("Foo::Bar")),
                sym(9, Kind::Method, "second", Some("Foo::Bar")),
                sym(10, Kind::Method, "third", Some("Foo::Bar")),
                sym(11, Kind::Method, "fourth", Some("Foo::Bar")),
                sym(13, Kind::Method, "initialize", Some("Foo::Bar")),
                sym(17, Kind::Smethod, "build", Some("Foo::Bar")),
                sym(22, Kind::Smethod, "cached", Some("Foo::Bar")),
                sym(28, Kind::Module, "Util", Some("Foo")),
                sym(29, Kind::Smethod, "helper", Some("Foo::Util")),
                sym(33, Kind::Class, "Foo::Baz", None),
                sym(34, Kind::Method, "qux", Some("Foo::Baz")),
            ]
        );
    }
}
