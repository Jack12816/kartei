//! Language registry, detection and shared extraction helpers.
//!
//! Every supported language owns a submodule with one embedded
//! tree-sitter definition query plus a small post-processing step
//! (scope building, filtering). Detection order is: filename rules,
//! then extension, then shebang for extensionless files.

pub mod bash;
pub mod c;
pub mod dockerfile;
pub mod erlang;
pub mod javascript;
pub mod make;
pub mod markdown;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod typescript;
pub mod yaml;

use std::collections::HashMap;

use anyhow::{Context, Result};
use tree_sitter::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

/// The closed symbol-kind vocabulary.
///
/// The kind names are part of the documented TSV contract; wrappers
/// may rely on them staying stable per release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A class-like type (class, struct, trait, interface, record).
    Class,
    /// A module or namespace.
    Module,
    /// An instance method or class-bound function.
    Method,
    /// A singleton method (eg. Ruby `def self.foo`).
    Smethod,
    /// A constant definition.
    Const,
    /// A free function or macro.
    Func,
    /// A make rule target.
    Target,
    /// A configuration key (YAML dotted path).
    Key,
    /// A markdown heading.
    Heading,
    /// A Dockerfile base image.
    Image,
    /// A Dockerfile build argument.
    Arg,
    /// An environment or build variable (Dockerfile ENV, shell and
    /// make assignments, top-level bindings).
    Env,
    /// A file entry (one per tracked file, named by its whole path).
    File,
}

impl Kind {
    /// All stable kind identifiers in declaration order — the single
    /// source for the `@kind` query sigils and their documentation.
    pub const ALL: &'static [&'static str] = &[
        "class", "module", "method", "smethod", "const", "func", "target",
        "key", "heading", "image", "arg", "env", "file",
    ];

    /// The result ranking of the kinds, most relevant first (the
    /// structural anchors class and module lead, data keys and
    /// images trail). The pick feed's first row renders next to the
    /// fzf prompt, so the most relevant kinds land in view.
    pub const RANKING: &'static [&'static str] = &[
        "class", "module", "file", "method", "func", "smethod", "const",
        "target", "arg", "heading", "env", "key", "image",
    ];

    /// The stable TSV identifier of the kind.
    ///
    /// @return the lowercase kind name
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Class => "class",
            Kind::Module => "module",
            Kind::Method => "method",
            Kind::Smethod => "smethod",
            Kind::Const => "const",
            Kind::Func => "func",
            Kind::Target => "target",
            Kind::Key => "key",
            Kind::Heading => "heading",
            Kind::Image => "image",
            Kind::Arg => "arg",
            Kind::Env => "env",
            Kind::File => "file",
        }
    }
}

/// A symbol extracted from a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// The 1-based start line of the definition.
    pub line: u32,
    /// The symbol kind.
    pub kind: Kind,
    /// The display name.
    pub name: String,
    /// The enclosing scope (eg. `Foo::Bar`), or +nil+ at top level.
    pub scope: Option<String>,
}

/// All supported languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Ruby sources.
    Ruby,
    /// Bash and POSIX shell scripts.
    Bash,
    /// Rust sources.
    Rust,
    /// Erlang sources and headers.
    Erlang,
    /// C sources and headers.
    C,
    /// Makefiles.
    Make,
    /// YAML configuration files.
    Yaml,
    /// Markdown documents.
    Markdown,
    /// Dockerfiles.
    Dockerfile,
    /// JavaScript sources (incl. JSX).
    Javascript,
    /// TypeScript sources.
    Typescript,
    /// TypeScript sources with JSX (TSX dialect).
    Tsx,
    /// Python sources.
    Python,
}

impl Lang {
    /// The stable language identifier stored in the database.
    ///
    /// @return the lowercase language name
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Ruby => "ruby",
            Lang::Bash => "bash",
            Lang::Rust => "rust",
            Lang::Erlang => "erlang",
            Lang::C => "c",
            Lang::Make => "make",
            Lang::Yaml => "yaml",
            Lang::Markdown => "markdown",
            Lang::Dockerfile => "dockerfile",
            Lang::Javascript => "javascript",
            Lang::Typescript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Python => "python",
        }
    }

    /// Detect the language of a file by name and extension.
    ///
    /// @param path the repo-relative file path
    /// @return the detected language, or +nil+ when only a shebang
    ///   check (or nothing) can decide
    pub fn detect(path: &str) -> Option<Lang> {
        let basename = path.rsplit('/').next().unwrap_or(path);

        // Filename rules take precedence over extensions
        match basename {
            "Makefile" | "GNUmakefile" | "makefile" => return Some(Lang::Make),
            "Rakefile" | "Gemfile" | "config.ru" => return Some(Lang::Ruby),
            "Dockerfile" => return Some(Lang::Dockerfile),
            _ => {}
        }
        if basename.starts_with("Dockerfile.") {
            return Some(Lang::Dockerfile);
        }

        let extension = basename.rsplit_once('.').map(|(_, ext)| ext)?;
        match extension {
            "rb" | "rake" | "gemspec" => Some(Lang::Ruby),
            "sh" | "bash" => Some(Lang::Bash),
            "rs" => Some(Lang::Rust),
            "erl" | "hrl" => Some(Lang::Erlang),
            "c" | "h" => Some(Lang::C),
            "mk" => Some(Lang::Make),
            "yml" | "yaml" => Some(Lang::Yaml),
            "md" | "markdown" => Some(Lang::Markdown),
            "dockerfile" => Some(Lang::Dockerfile),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::Javascript),
            "ts" | "mts" | "cts" => Some(Lang::Typescript),
            "tsx" => Some(Lang::Tsx),
            "py" | "pyi" => Some(Lang::Python),
            _ => None,
        }
    }

    /// Detect the language of an extensionless file by its shebang.
    ///
    /// @param first_line the first line of the file
    /// @return the detected language, or +nil+ for non-scripts
    pub fn from_shebang(first_line: &str) -> Option<Lang> {
        let line = first_line.strip_prefix("#!")?;
        // Take the interpreter, resolving `/usr/bin/env <interp>`
        let mut words = line.split_whitespace();
        let mut interp = words.next()?.rsplit('/').next()?;
        if interp == "env" {
            interp = words.next()?;
        }
        // Strip version suffixes like python3 or bash5
        let interp = interp
            .trim_end_matches(|chr: char| chr.is_ascii_digit() || chr == '.');
        match interp {
            "ruby" => Some(Lang::Ruby),
            "bash" | "sh" | "dash" | "zsh" => Some(Lang::Bash),
            "python" => Some(Lang::Python),
            _ => None,
        }
    }

    /// Extract all symbols from a source file.
    ///
    /// @param source the raw file contents
    /// @param resolve_yaml whether Yaml anchors, aliases and merge
    ///   keys resolve into inherited key symbols
    /// @return the extracted symbols
    /// @raise when the grammar or query fails to load
    pub fn extract(
        self,
        source: &[u8],
        resolve_yaml: bool,
    ) -> Result<Vec<Symbol>> {
        match self {
            Lang::Ruby => ruby::extract(source),
            Lang::Bash => bash::extract(source),
            Lang::Rust => rust::extract(source),
            Lang::Erlang => erlang::extract(source),
            Lang::C => c::extract(source),
            Lang::Make => make::extract(source),
            Lang::Yaml => yaml::extract(source, resolve_yaml),
            Lang::Markdown => markdown::extract(source),
            Lang::Dockerfile => dockerfile::extract(source),
            Lang::Javascript => javascript::extract(source),
            Lang::Typescript => typescript::extract(source),
            Lang::Tsx => typescript::extract_tsx(source),
            Lang::Python => python::extract(source),
        }
    }
}

/// Parse a source file with the given grammar.
///
/// @param language the tree-sitter grammar
/// @param source the raw file contents
/// @return the parse tree
/// @raise when the parser rejects the grammar or the source
pub fn parse(language: &Language, source: &[u8]) -> Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .context("failed to load the grammar")?;
    parser
        .parse(source, None)
        .context("failed to parse the source")
}

/// Fetch the cached compilation of a definition query.
///
/// Query compilation is expensive relative to parsing a single file,
/// so compiled queries are memoized for the process lifetime, keyed by
/// the address of their embedded source (all callers pass +'static+
/// +include_str!+ constants). The handful of queries is deliberately
/// leaked — they live until exit anyway.
///
/// @param language the tree-sitter grammar
/// @param query_src the static S-expression query source
/// @return the compiled query
/// @raise when the query does not compile against the grammar
fn compiled_query(
    language: &Language,
    query_src: &'static str,
) -> Result<&'static Query> {
    static CACHE: std::sync::Mutex<
        std::cell::OnceCell<HashMap<usize, &'static Query>>,
    > = std::sync::Mutex::new(std::cell::OnceCell::new());

    let mut guard = CACHE.lock().expect("query cache poisoned");
    guard.get_or_init(HashMap::new);
    let cache = guard.get_mut().expect("query cache initialized");

    let key = query_src.as_ptr() as usize;
    if let Some(query) = cache.get(&key) {
        return Ok(query);
    }
    let query = Query::new(language, query_src)
        .context("failed to compile the definition query")?;
    let query: &'static Query = Box::leak(Box::new(query));
    cache.insert(key, query);
    Ok(query)
}

/// Run a definition query and hand every match to the given builder.
///
/// The builder receives the capture name and node per capture and
/// returns the symbols to record; matches are visited in source order.
///
/// @param language the tree-sitter grammar
/// @param query_src the static S-expression query source
/// @param source the raw file contents
/// @yield [capture, node] every query capture
/// @return the collected symbols
pub fn run_query<F>(
    language: &Language,
    query_src: &'static str,
    source: &[u8],
    mut builder: F,
) -> Result<Vec<Symbol>>
where
    F: FnMut(&str, Node<'_>) -> Option<Symbol>,
{
    let tree = parse(language, source)?;
    let query = compiled_query(language, query_src)?;
    let names = query.capture_names();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source);
    let mut symbols = Vec::new();
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            let name = names[capture.index as usize];
            if let Some(symbol) = builder(name, capture.node) {
                symbols.push(symbol);
            }
        }
    }
    Ok(symbols)
}

/// Fetch the UTF-8 text of a node.
///
/// @param node the syntax node
/// @param source the raw file contents
/// @return the node text, lossily decoded
pub fn node_text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()]).into_owned()
}

/// The 1-based start line of a node.
///
/// @param node the syntax node
/// @return the line number
pub fn line_of(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_every_kind_exactly_once() {
        let mut ranked: Vec<&str> = Kind::RANKING.to_vec();
        let mut all: Vec<&str> = Kind::ALL.to_vec();
        ranked.sort_unstable();
        all.sort_unstable();
        assert_eq!(ranked, all);
    }

    #[test]
    fn detects_by_extension() {
        assert_eq!(Lang::detect("app/models/user.rb"), Some(Lang::Ruby));
    }

    #[test]
    fn detects_makefiles_by_name() {
        assert_eq!(Lang::detect("sub/Makefile"), Some(Lang::Make));
    }

    #[test]
    fn detects_dockerfile_variants() {
        assert_eq!(Lang::detect("Dockerfile.dev"), Some(Lang::Dockerfile));
    }

    #[test]
    fn detects_tsx_separately() {
        assert_eq!(Lang::detect("ui/App.tsx"), Some(Lang::Tsx));
    }

    #[test]
    fn detects_gemspecs_as_ruby() {
        assert_eq!(Lang::detect("kartei.gemspec"), Some(Lang::Ruby));
    }

    #[test]
    fn leaves_unknown_extensions_undetected() {
        assert_eq!(Lang::detect("logo.svg"), None);
    }

    #[test]
    fn detects_ruby_shebangs() {
        assert_eq!(Lang::from_shebang("#!/usr/bin/env ruby"), Some(Lang::Ruby));
    }

    #[test]
    fn detects_plain_sh_shebangs() {
        assert_eq!(Lang::from_shebang("#!/bin/sh"), Some(Lang::Bash));
    }

    #[test]
    fn detects_versioned_python_shebangs() {
        assert_eq!(
            Lang::from_shebang("#!/usr/bin/env python3"),
            Some(Lang::Python)
        );
    }

    #[test]
    fn rejects_non_script_shebangs() {
        assert_eq!(Lang::from_shebang("#!/usr/bin/env node"), None);
    }

    #[test]
    fn rejects_regular_lines() {
        assert_eq!(Lang::from_shebang("# frozen_string_literal: true"), None);
    }
}
