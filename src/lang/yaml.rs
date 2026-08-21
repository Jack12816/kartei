//! Yaml symbol extraction.
//!
//! Extraction walks the parse tree manually instead of running a
//! query: every mapping key needs its dotted path from the document
//! root (eg. `server.ports`), which only a stateful descent can
//! build. Sequences never extend the path, so keys inside a sequence
//! of mappings continue the parent path. Flow mappings (`{a: 1}`) are
//! handled the same way as block mappings since both expose keyed
//! pair nodes. All documents of a multi-document stream are walked,
//! each with its own anchor scope. The dotted path already encodes
//! the nesting, so every symbol is scope-free.
//!
//! Anchors, aliases and merge keys resolve like a Yaml loader would:
//! an alias value (`copy: *shared`) contributes the anchored
//! mapping's keys under the aliasing path (`copy.retries`), and a
//! merge key (`<<: *defaults`, also the `[*a, *b]` sequence form)
//! inlines the referenced mappings — so `production.pool` is indexed
//! even when it is only written under `defaults:`. Yaml merging is
//! shallow: a mapping's own keys always win, and among several merge
//! sources the first wins. Inherited keys keep their origin line —
//! where the key is actually written and would be edited. A
//! visited-anchor stack guards recursive references. Resolution is
//! gated by the index.resolve_yaml configuration flag; disabled,
//! only spelled-out keys are extracted and shared content
//! contributes nothing.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use tree_sitter::Node;

use super::{Kind, Symbol, line_of, node_text, parse};

/// The maximum number of dotted-path segments; deeper keys are
/// omitted.
const MAX_DEPTH: usize = 10;

/// Extract all symbols from a Yaml document.
///
/// @param source the raw file contents
/// @param resolve whether anchors, aliases and merge keys resolve
///   into inherited key symbols; disabled, only spelled-out keys
///   are extracted
/// @return the extracted symbols
/// @raise when the grammar fails to load
pub fn extract(source: &[u8], resolve: bool) -> Result<Vec<Symbol>> {
    let language = tree_sitter_yaml::LANGUAGE.into();
    let tree = parse(&language, source)?;
    let mut symbols = Vec::new();
    // Each document of a stream owns its own anchor scope
    let mut cursor = tree.root_node().walk();
    for document in tree.root_node().named_children(&mut cursor) {
        let mut resolver = Resolver {
            source,
            anchors: match resolve {
                true => collect_anchors(document, source),
                false => HashMap::new(),
            },
            visiting: Vec::new(),
        };
        let mut path = Vec::new();
        resolver.walk(document, &mut path, &mut symbols);
    }
    Ok(symbols)
}

/// Collect the anchor table of one document.
///
/// The anchored value is the anchor label's parent node (the
/// block/flow node the label decorates), so walking it later renders
/// the shared content.
///
/// @param document the document node
/// @param source the raw file contents
/// @return the anchored value nodes by anchor name
fn collect_anchors<'a>(
    document: Node<'a>,
    source: &[u8],
) -> HashMap<String, Node<'a>> {
    let mut anchors = HashMap::new();
    let mut stack = vec![document];
    while let Some(node) = stack.pop() {
        if node.kind() == "anchor"
            && let (Some(label), Some(target)) =
                (node.named_child(0), node.parent())
        {
            anchors.insert(node_text(label, source).trim().to_string(), target);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    anchors
}

/// The stateful tree walker resolving anchors, aliases and merges.
struct Resolver<'a> {
    /// The raw file contents.
    source: &'a [u8],
    /// The anchored value nodes of the current document.
    anchors: HashMap<String, Node<'a>>,
    /// The anchor names currently being expanded (cycle guard).
    visiting: Vec<String>,
}

impl<'a> Resolver<'a> {
    /// Walk a node, recording every mapping key on the way down.
    ///
    /// @param node the syntax node to descend into
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    fn walk(
        &mut self,
        node: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
    ) {
        match node.kind() {
            // The anchor label itself names shared content, not a key
            "anchor" => {}
            "alias" => self.alias(node, path, symbols),
            "block_mapping" | "flow_mapping" => {
                self.mapping(node, path, symbols);
            }
            // Pairs outside a mapping (eg. inside a flow sequence)
            "block_mapping_pair" | "flow_pair" => {
                self.record(node, path, symbols);
            }
            _ => {
                // Descend through documents, sequences and flow nodes
                // alike; sequences add no path segment by design
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.walk(child, path, symbols);
                }
            }
        }
    }

    /// Resolve an alias value to its anchored content.
    ///
    /// The anchored node is walked under the current path, so
    /// `copy: *shared` contributes `copy.retries` with the origin
    /// line of the shared key.
    ///
    /// @param node the alias node
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    fn alias(
        &mut self,
        node: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
    ) {
        let Some(label) = node.named_child(0) else {
            return;
        };
        let name = node_text(label, self.source).trim().to_string();
        let Some(&target) = self.anchors.get(&name) else {
            return;
        };
        // Recursive references (&a referencing itself) must not loop
        if self.visiting.contains(&name) {
            return;
        }
        self.visiting.push(name);
        self.walk(target, path, symbols);
        self.visiting.pop();
    }

    /// Walk a mapping: own pairs first, merge sources after.
    ///
    /// Yaml merging is shallow — the mapping's own keys always win,
    /// regardless of where the merge key sits — so the own key names
    /// pre-populate the seen set before any merge source expands.
    ///
    /// @param node the mapping node
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    fn mapping(
        &mut self,
        node: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
    ) {
        let mut cursor = node.walk();
        let pairs: Vec<Node<'a>> = node
            .named_children(&mut cursor)
            .filter(|child| {
                matches!(child.kind(), "block_mapping_pair" | "flow_pair")
            })
            .collect();

        let mut seen: HashSet<String> = pairs
            .iter()
            .filter_map(|pair| self.pair_name(*pair))
            .filter(|name| name != "<<")
            .collect();

        for pair in &pairs {
            if self.pair_name(*pair).as_deref() != Some("<<") {
                self.record(*pair, path, symbols);
            }
        }
        for pair in &pairs {
            if self.pair_name(*pair).as_deref() == Some("<<")
                && let Some(value) = pair.child_by_field_name("value")
            {
                self.merge(value, path, symbols, &mut seen);
            }
        }
    }

    /// Expand the sources of one merge key into the current mapping.
    ///
    /// The value is a single alias or a sequence of aliases; earlier
    /// sources win over later ones, so the shared seen set makes the
    /// first occurrence of a key stick.
    ///
    /// @param value the merge key's value node
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    /// @param seen the key names already claimed at this level
    fn merge(
        &mut self,
        value: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
        seen: &mut HashSet<String>,
    ) {
        for alias in aliases_within(value) {
            let Some(label) = alias.named_child(0) else {
                continue;
            };
            let name = node_text(label, self.source).trim().to_string();
            let Some(&target) = self.anchors.get(&name) else {
                continue;
            };
            if self.visiting.contains(&name) {
                continue;
            }
            let Some(mapping) = mapping_within(target) else {
                continue;
            };
            self.visiting.push(name);
            self.merge_into(mapping, path, symbols, seen);
            self.visiting.pop();
        }
    }

    /// Inline one merge source mapping at the current level.
    ///
    /// The source's own pairs come first (they beat its own nested
    /// merges), unclaimed keys record under the inheriting path with
    /// their origin line, and nested merge keys chain further.
    ///
    /// @param mapping the anchored source mapping
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    /// @param seen the key names already claimed at this level
    fn merge_into(
        &mut self,
        mapping: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
        seen: &mut HashSet<String>,
    ) {
        let mut cursor = mapping.walk();
        let pairs: Vec<Node<'a>> = mapping
            .named_children(&mut cursor)
            .filter(|child| {
                matches!(child.kind(), "block_mapping_pair" | "flow_pair")
            })
            .collect();

        for pair in &pairs {
            if let Some(name) = self.pair_name(*pair)
                && name != "<<"
                && !seen.contains(&name)
            {
                seen.insert(name);
                self.record(*pair, path, symbols);
            }
        }
        for pair in &pairs {
            if self.pair_name(*pair).as_deref() == Some("<<")
                && let Some(value) = pair.child_by_field_name("value")
            {
                self.merge(value, path, symbols, seen);
            }
        }
    }

    /// Record a mapping pair's key and descend into its value.
    ///
    /// Keys nested deeper than the path limit are omitted entirely,
    /// along with their subtrees, since every deeper key would only
    /// grow the path further. Merge keys never record — they are
    /// inheritance plumbing, expanded by the mapping walk.
    ///
    /// @param node the pair node
    /// @param path the key segments collected so far
    /// @param symbols the symbol collector
    fn record(
        &mut self,
        node: Node<'a>,
        path: &mut Vec<String>,
        symbols: &mut Vec<Symbol>,
    ) {
        let Some(key) = node.child_by_field_name("key") else {
            return;
        };
        let name = key_name(key, self.source);
        if name == "<<" {
            return;
        }
        path.push(name);
        if path.len() <= MAX_DEPTH {
            symbols.push(Symbol {
                line: line_of(key),
                kind: Kind::Key,
                name: path.join("."),
                scope: None,
            });
            if let Some(value) = node.child_by_field_name("value") {
                self.walk(value, path, symbols);
            }
        }
        path.pop();
    }

    /// Fetch the display name of a pair's key.
    ///
    /// @param pair the pair node
    /// @return the bare key text, or +nil+ for keyless pairs
    fn pair_name(&self, pair: Node<'a>) -> Option<String> {
        pair.child_by_field_name("key")
            .map(|key| key_name(key, self.source))
    }
}

/// Collect the alias nodes within a merge value, in source order.
///
/// @param node the merge key's value node
/// @return the alias nodes
fn aliases_within(node: Node<'_>) -> Vec<Node<'_>> {
    let mut aliases = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "alias" {
            aliases.push(node);
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> =
            node.named_children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    aliases
}

/// Find the mapping node within an anchored value node.
///
/// @param node the anchored value node
/// @return the mapping node, or +nil+ for non-mapping anchors
fn mapping_within(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(node.kind(), "block_mapping" | "flow_mapping") {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // The anchor label never contains the shared content
        if child.kind() == "anchor" {
            continue;
        }
        if let Some(found) = mapping_within(child) {
            return Some(found);
        }
    }
    None
}

/// Fetch the display name of a mapping key.
///
/// Surrounding quotes of quoted keys are stripped; escape sequences
/// inside stay as written since configuration keys rarely carry any.
///
/// @param node the key node
/// @param source the raw file contents
/// @return the bare key text
fn key_name(node: Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);
    let text = text.trim();
    for quote in ['"', '\''] {
        let inner = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote));
        if let Some(inner) = inner {
            return inner.to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Yaml fixture exercised by all extraction tests.
    const FIXTURE: &[u8] =
        include_bytes!("../../tests/fixtures/yaml/sample.yml");

    /// Extract all symbols from the fixture, resolution enabled.
    ///
    /// @return the extracted symbols
    fn symbols() -> Vec<Symbol> {
        extract(FIXTURE, true).unwrap()
    }

    /// Extract all symbols from the fixture, resolution disabled.
    ///
    /// @return the extracted symbols
    fn unresolved() -> Vec<Symbol> {
        extract(FIXTURE, false).unwrap()
    }

    /// Build a symbol literal for the assertions.
    ///
    /// @param line the 1-based line
    /// @param name the dotted key path
    /// @return the scope-free key symbol
    fn sym(line: u32, name: &str) -> Symbol {
        Symbol {
            line,
            kind: Kind::Key,
            name: name.to_string(),
            scope: None,
        }
    }

    #[test]
    fn extracts_nested_keys_as_dotted_paths() {
        assert!(symbols().contains(&sym(3, "server.host")));
    }

    #[test]
    fn continues_the_parent_path_inside_sequences() {
        assert!(symbols().contains(&sym(10, "jobs.steps.run")));
    }

    #[test]
    fn keeps_keys_at_the_depth_limit() {
        assert!(symbols().contains(&sym(25, "deep.a.b.c.d.e.f.g.h.i")));
    }

    #[test]
    fn omits_keys_below_the_depth_limit() {
        assert!(!symbols().iter().any(|found| found.name.ends_with(".j")));
    }

    #[test]
    fn resolves_alias_values_under_the_aliasing_path() {
        assert!(symbols().contains(&sym(14, "copy.retries")));
    }

    #[test]
    fn resolves_merge_keys_under_the_inheriting_path() {
        assert!(symbols().contains(&sym(14, "merged.retries")));
    }

    #[test]
    fn keeps_explicit_keys_over_merged_ones() {
        assert!(symbols().contains(&sym(32, "override.retries")));
        assert!(!symbols().contains(&sym(14, "override.retries")));
    }

    #[test]
    fn resolves_merge_chains_transitively() {
        assert!(symbols().contains(&sym(35, "twice.extra")));
        assert!(symbols().contains(&sym(14, "twice.retries")));
    }

    #[test]
    fn survives_recursive_merge_references() {
        assert!(
            !symbols()
                .iter()
                .any(|found| found.name.starts_with("loop."))
        );
    }

    #[test]
    fn extracts_all_documents_of_a_stream() {
        assert!(symbols().contains(&sym(41, "second")));
    }

    #[test]
    fn skips_shared_content_when_resolution_is_disabled() {
        assert!(
            !unresolved()
                .iter()
                .any(|found| found.name == "copy.retries")
        );
    }

    #[test]
    fn keeps_spelled_out_keys_when_resolution_is_disabled() {
        assert!(unresolved().contains(&sym(29, "merged.own")));
    }

    #[test]
    fn skips_merge_keys() {
        assert!(!symbols().iter().any(|found| found.name.contains("<<")));
    }

    #[test]
    fn extracts_the_exact_symbol_list() {
        assert_eq!(
            symbols(),
            vec![
                sym(1, "name"),
                sym(2, "server"),
                sym(3, "server.host"),
                sym(4, "server.ports"),
                sym(7, "jobs"),
                sym(8, "jobs.name"),
                sym(9, "jobs.steps"),
                sym(10, "jobs.steps.run"),
                sym(11, "jobs.name"),
                sym(12, "flow"),
                sym(12, "flow.alpha"),
                sym(12, "flow.beta"),
                sym(13, "base"),
                sym(14, "base.retries"),
                sym(15, "copy"),
                sym(14, "copy.retries"),
                sym(16, "deep"),
                sym(17, "deep.a"),
                sym(18, "deep.a.b"),
                sym(19, "deep.a.b.c"),
                sym(20, "deep.a.b.c.d"),
                sym(21, "deep.a.b.c.d.e"),
                sym(22, "deep.a.b.c.d.e.f"),
                sym(23, "deep.a.b.c.d.e.f.g"),
                sym(24, "deep.a.b.c.d.e.f.g.h"),
                sym(25, "deep.a.b.c.d.e.f.g.h.i"),
                sym(27, "merged"),
                sym(29, "merged.own"),
                sym(14, "merged.retries"),
                sym(30, "override"),
                sym(32, "override.retries"),
                sym(33, "chain"),
                sym(35, "chain.extra"),
                sym(14, "chain.retries"),
                sym(36, "twice"),
                sym(35, "twice.extra"),
                sym(14, "twice.retries"),
                sym(38, "loop"),
                sym(41, "second"),
            ]
        );
    }
}
