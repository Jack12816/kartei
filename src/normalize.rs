//! Symbol and path normalization.
//!
//! Normalization is the heart of the all-casing feature: symbols, file
//! paths and query terms are all folded into one canonical snake_case
//! form at index time, so `FooBar`, `foo_bar`, `foo-bar` and `Foo::Bar`
//! collapse to the same matching key and no query-time variant
//! expansion is needed. Every token is additionally singularized, so
//! plural and singular forms (`UserPolicies` == `user_policy`) match
//! as well.

/// Whether the character separates tokens outright.
fn is_separator(chr: char) -> bool {
    matches!(chr, '_' | '-' | '.' | '/' | ':' | ' ' | '\t')
}

/// One normalized token with its byte span in the raw input.
#[derive(Debug, PartialEq, Eq)]
pub struct Token {
    /// The lowercase, singularized token text.
    pub norm: String,
    /// The token's start byte offset in the raw input.
    pub start: usize,
    /// The token's end byte offset in the raw input (exclusive).
    pub end: usize,
}

/// Normalize a symbol, file path or query term.
///
/// Splits the input into tokens on separator characters (`_ - . / :`
/// and whitespace), on lower-to-upper case transitions (`fooBar`), on
/// acronym boundaries (`HTTPServer` -> `http` + `server`) and on
/// letter-digit transitions, then lowercases and singularizes the
/// tokens and joins them with underscores.
///
/// @param raw the raw symbol text
/// @return the canonical snake_case form
pub fn normalize(raw: &str) -> String {
    tokens(raw)
        .into_iter()
        .map(|token| token.norm)
        .collect::<Vec<_>>()
        .join("_")
}

/// Tokenize a raw string into normalized tokens with byte spans.
///
/// This is the tokenizer behind +normalize+, kept span-aware so
/// display highlighting can map normalized matches back onto the raw
/// text.
///
/// @param raw the raw symbol text
/// @return the normalized tokens in input order
pub fn tokens(raw: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut start = 0usize;
    let chars: Vec<(usize, char)> = raw.char_indices().collect();

    for (idx, &(offset, chr)) in chars.iter().enumerate() {
        if is_separator(chr) {
            flush(&mut tokens, &mut current, start, offset);
            continue;
        }

        if !current.is_empty() {
            let prev = chars[idx - 1].1;
            let next = chars.get(idx + 1).map(|&(_, next)| next);
            if boundary(prev, chr, next) {
                flush(&mut tokens, &mut current, start, offset);
            }
        }

        if current.is_empty() {
            start = offset;
        }
        current.extend(chr.to_lowercase());
    }
    flush(&mut tokens, &mut current, start, raw.len());
    tokens
}

/// Locate the raw byte spans covered by normalized query words.
///
/// The raw text is tokenized like +normalize+ does, every occurrence
/// of a word in the joined normalized form marks the tokens it
/// touches, and runs of adjacent marked tokens merge into one span
/// (the separators between them included). The granularity is whole
/// tokens on purpose: singularization makes an exact character
/// mapping impossible (`policies` -> `policy`).
///
/// @param raw the raw display text
/// @param words the normalized query words
/// @return the sorted, non-overlapping `(start, end)` byte spans
pub fn match_spans(raw: &str, words: &[&str]) -> Vec<(usize, usize)> {
    let tokens = tokens(raw);

    // Rebuild the joined normalized form with per-token spans in it,
    // so word occurrences can be located exactly like the database
    // LIKE filter located them
    let mut joined = String::new();
    let mut norm_spans = Vec::with_capacity(tokens.len());
    for token in &tokens {
        if !joined.is_empty() {
            joined.push('_');
        }
        let start = joined.len();
        joined.push_str(&token.norm);
        norm_spans.push((start, joined.len()));
    }

    // Mark every token a boundary-anchored occurrence of any word
    // touches — mid-token hits are not matches and must not paint
    let mut marked = vec![false; tokens.len()];
    for word in words.iter().filter(|word| !word.is_empty()) {
        let mut from = 0;
        while let Some(pos) = joined[from..].find(word) {
            let (start, end) = (from + pos, from + pos + word.len());
            if start == 0 || joined.as_bytes()[start - 1] == b'_' {
                for (idx, &(t_start, t_end)) in norm_spans.iter().enumerate() {
                    if t_start < end && start < t_end {
                        marked[idx] = true;
                    }
                }
            }
            from = start + 1;
        }
    }

    // Merge runs of adjacent marked tokens into one raw span
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        if !marked[idx] {
            continue;
        }
        match spans.last_mut() {
            Some(last) if idx > 0 && marked[idx - 1] => last.1 = token.end,
            _ => spans.push((token.start, token.end)),
        }
    }
    spans
}

/// Whether the needle occurs starting at a token boundary.
///
/// A boundary is the string start or the position right after an
/// underscore in the normalized form; the occurrence may end
/// mid-token, so `pain` finds `painless` but `ai` does not.
///
/// @param haystack the normalized haystack
/// @param needle the normalized needle
/// @return whether a boundary-anchored occurrence exists
pub fn contains_at_boundary(haystack: &str, needle: &str) -> bool {
    !needle.is_empty()
        && haystack
            .match_indices(needle)
            .any(|(idx, _)| idx == 0 || haystack.as_bytes()[idx - 1] == b'_')
}

/// Whether the needle occurs as a complete token run.
///
/// Both ends of the occurrence must sit on token boundaries, so
/// `ai` matches the token `ai` but never the start of `aid`; a
/// multi-token needle (`user_policy`) matches the exact token run.
///
/// @param haystack the normalized haystack
/// @param needle the normalized needle
/// @return whether a whole-token occurrence exists
pub fn contains_whole_tokens(haystack: &str, needle: &str) -> bool {
    !needle.is_empty()
        && haystack.match_indices(needle).any(|(idx, _)| {
            let end = idx + needle.len();
            (idx == 0 || haystack.as_bytes()[idx - 1] == b'_')
                && (end == haystack.len() || haystack.as_bytes()[end] == b'_')
        })
}

/// Singularize one lowercase token with naive English plural rules.
///
/// The rules run identically at index and query time, so a canonical
/// form never has to be correct English — both sides just have to
/// agree. Two constraints shape the rules:
///
///   * Substring matching tolerates keeping too many characters
///     (`buses` -> `buse` still contains `bus`) but never stripping
///     too many (`cases` -> `cas` would lose `case`), so the plain
///     rule strips exactly one trailing `s`.
///   * `ie` and `ies` endings unify to `y`, because `cookie` and
///     `cookies` cannot agree through suffix-stripping alone.
///
/// @param token the lowercase token
/// @return the canonical singular-ish token
fn singularize(token: String) -> String {
    if token.len() < 4 || token.ends_with("ss") {
        return token;
    }
    if let Some(stem) = token.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if let Some(stem) = token.strip_suffix("ie") {
        return format!("{stem}y");
    }
    match token.strip_suffix('s') {
        Some(stem) => stem.to_string(),
        None => token,
    }
}

/// Whether a token boundary lies between the previous and the current
/// character.
///
/// @param prev the previous character
/// @param chr the current character
/// @param next the following character, when any
/// @return whether to start a new token at the current character
fn boundary(prev: char, chr: char, next: Option<char>) -> bool {
    // Lower-to-upper transition (fooBar)
    if prev.is_lowercase() && chr.is_uppercase() {
        return true;
    }
    // Acronym boundary: the last upper of an upper run followed by a
    // lower belongs to the next token (HTTPServer -> http server)
    if prev.is_uppercase()
        && chr.is_uppercase()
        && next.is_some_and(|n| n.is_lowercase())
    {
        return true;
    }
    // Letter-digit transitions (utf8Decoder -> utf 8 decoder)
    if prev.is_alphabetic() && chr.is_ascii_digit() {
        return true;
    }
    if prev.is_ascii_digit() && chr.is_alphabetic() {
        return true;
    }
    false
}

/// Push the collected token onto the list and reset the collector.
///
/// @param tokens the token list collected so far
/// @param current the token collector to flush
/// @param start the token's start byte offset in the raw input
/// @param end the token's end byte offset in the raw input
fn flush(
    tokens: &mut Vec<Token>,
    current: &mut String,
    start: usize,
    end: usize,
) {
    if !current.is_empty() {
        tokens.push(Token {
            norm: singularize(std::mem::take(current)),
            start,
            end,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_camel_case() {
        assert_eq!(normalize("FooBar"), "foo_bar");
    }

    #[test]
    fn normalizes_lower_camel_case() {
        assert_eq!(normalize("fooBar"), "foo_bar");
    }

    #[test]
    fn normalizes_kebab_case() {
        assert_eq!(normalize("foo-bar"), "foo_bar");
    }

    #[test]
    fn normalizes_scope_resolution() {
        assert_eq!(normalize("Foo::Bar"), "foo_bar");
    }

    #[test]
    fn normalizes_acronyms() {
        assert_eq!(normalize("HTTPServer"), "http_server");
    }

    #[test]
    fn normalizes_dotted_paths() {
        assert_eq!(normalize("a.b.c"), "a_b_c");
    }

    #[test]
    fn normalizes_filenames() {
        assert_eq!(normalize("created_job.rb"), "created_job_rb");
    }

    #[test]
    fn normalizes_digit_transitions() {
        assert_eq!(normalize("utf8Decoder"), "utf_8_decoder");
    }

    #[test]
    fn normalizes_snake_case_identity() {
        assert_eq!(normalize("already_snake"), "already_snake");
    }

    #[test]
    fn normalizes_upper_snake() {
        assert_eq!(normalize("MAX_RETRIES"), "max_retry");
    }

    #[test]
    fn normalizes_empty_input() {
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn singularizes_plural_tokens() {
        assert_eq!(normalize("UserPolicies"), "user_policy");
    }

    #[test]
    fn singularizes_plain_plural_s() {
        assert_eq!(normalize("jobs"), "job");
    }

    #[test]
    fn unifies_ie_and_ies_endings() {
        assert_eq!(normalize("cookie"), normalize("cookies"));
    }

    #[test]
    fn keeps_double_s_endings() {
        assert_eq!(normalize("business"), "business");
    }

    #[test]
    fn keeps_short_tokens_untouched() {
        assert_eq!(normalize("its"), "its");
    }

    #[test]
    fn overretains_instead_of_overstripping() {
        // `classes` may stay imperfect (classe) — it still contains
        // the singular `class`, which substring matching relies on
        assert_eq!(normalize("classes"), "classe");
    }

    #[test]
    fn normalizes_whole_paths_across_components() {
        let norm = normalize("app/jobs/lead_manager/kafka/created_job.rb");
        assert_eq!(norm, "app_job_lead_manager_kafka_created_job_rb");
    }

    #[test]
    fn matches_path_fragments_case_insensitively() {
        let norm = normalize("ndp/config/businessCases/find.rb");
        assert!(norm.contains(&normalize("config/businessCase")));
    }

    #[test]
    fn tokenizes_with_byte_spans() {
        assert_eq!(
            tokens("FooBar"),
            vec![
                Token {
                    norm: "foo".into(),
                    start: 0,
                    end: 3
                },
                Token {
                    norm: "bar".into(),
                    start: 3,
                    end: 6
                }
            ]
        );
    }

    #[test]
    fn tokenizes_spans_across_separators() {
        let spans: Vec<(usize, usize)> = tokens("lib/user.rb")
            .into_iter()
            .map(|token| (token.start, token.end))
            .collect();
        assert_eq!(spans, vec![(0, 3), (4, 8), (9, 11)]);
    }

    #[test]
    fn spans_whole_camel_case_matches() {
        assert_eq!(match_spans("UserPolicy", &["user_policy"]), vec![(0, 10)]);
    }

    #[test]
    fn spans_singularized_matches() {
        assert_eq!(match_spans("UserPolicies", &["policy"]), vec![(4, 12)]);
    }

    #[test]
    fn spans_separate_word_matches_individually() {
        assert_eq!(match_spans("app/user/find.rb", &["find"]), vec![(9, 13)]);
    }

    #[test]
    fn spans_merge_adjacent_matched_tokens() {
        assert_eq!(match_spans("lib/user.rb", &["lib_user"]), vec![(0, 8)]);
    }

    #[test]
    fn spans_nothing_without_a_match() {
        assert!(match_spans("UserPolicy", &["nope"]).is_empty());
    }

    #[test]
    fn spans_nothing_for_mid_token_occurrences() {
        assert!(match_spans("painless", &["ai"]).is_empty());
    }

    #[test]
    fn finds_needles_at_the_string_start() {
        assert!(contains_at_boundary("pain_free", "pain"));
    }

    #[test]
    fn finds_needles_after_token_boundaries() {
        assert!(contains_at_boundary("no_pain_free", "pain"));
    }

    #[test]
    fn finds_needle_prefixes_of_tokens() {
        assert!(contains_at_boundary("painless_job", "pain"));
    }

    #[test]
    fn rejects_mid_token_needles() {
        assert!(!contains_at_boundary("painless", "ai"));
    }

    #[test]
    fn finds_whole_token_needles() {
        assert!(contains_whole_tokens("ai_prompt_release", "ai"));
    }

    #[test]
    fn finds_whole_multi_token_needles() {
        assert!(contains_whole_tokens("x_user_policy_y", "user_policy"));
    }

    #[test]
    fn rejects_token_prefixes_as_whole_tokens() {
        assert!(!contains_whole_tokens("finder_x", "find"));
    }
}
