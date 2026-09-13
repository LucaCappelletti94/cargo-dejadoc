//! Canonical form of a doctest body.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use quote::ToTokens;
/// Canonical form of a doctest body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical {
    /// Canonical text: deterministic token-stream string, or the collapsed
    /// text fallback.
    pub text: String,
    /// True when no `syn` parse succeeded and the text fallback was used.
    pub unparsed: bool,
    /// Token count for `min-tokens` filtering.
    pub tokens: usize,
}

/// Compute the canonical form of a doctest body.
#[must_use]
pub fn canonicalize(code: &str) -> Canonical {
    // rustdoc drops any line whose first non-whitespace character is `#`.
    let body: String = code
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if let Ok(file) = syn::parse_str::<syn::File>(&body) {
        return from_stream(file.to_token_stream());
    }
    if let Ok(block) = syn::parse_str::<syn::Block>(&format!("{{\n{body}\n}}")) {
        return from_stream(block.to_token_stream());
    }
    if let Ok(expr) = syn::parse_str::<syn::Expr>(&body) {
        return from_stream(expr.to_token_stream());
    }
    // Not Rust: keep the visible text, collapsed to single spaces.
    let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Canonical {
        text: text.clone(),
        unparsed: true,
        tokens: text.split_whitespace().count(),
    }
}

fn from_stream(stream: proc_macro2::TokenStream) -> Canonical {
    let text = stream.to_string();
    Canonical {
        text,
        unparsed: false,
        tokens: count_tokens(stream),
    }
}

/// Number of leaf tokens in a token stream.
fn count_tokens(stream: proc_macro2::TokenStream) -> usize {
    stream.into_iter().map(count_tree).sum()
}

fn count_tree(tree: proc_macro2::TokenTree) -> usize {
    match tree {
        proc_macro2::TokenTree::Group(group) => count_tokens(group.stream()),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_body_parses() {
        let a = canonicalize("fn main() { }");
        assert!(!a.unparsed);
        assert_ne!(a.text, "");
        assert!(a.tokens > 0);
    }

    #[test]
    fn comment_and_whitespace_drift_collapses() {
        let a = canonicalize("fn main() { }\n// a comment");
        let b = canonicalize("fn main(){}  /* other */  \n");
        assert_eq!(a.text, b.text);
        assert_eq!(a.tokens, b.tokens);
    }

    #[test]
    fn statement_body_parses() {
        let c = canonicalize("let x = 1;\nlet y = x + 1;\n");
        assert!(!c.unparsed);
    }

    #[test]
    fn bare_expression_parses() {
        let c = canonicalize("1 + 1");
        assert!(!c.unparsed);
        assert_eq!(c.tokens, 3);
    }

    #[test]
    fn hidden_lines_stripped() {
        let a = canonicalize("# hidden setup\nlet x = 1;\n");
        let b = canonicalize("let x = 1;\n");
        assert_eq!(a.text, b.text);
        assert!(!a.unparsed);
    }

    #[test]
    fn indented_hidden_lines_stripped() {
        let a = canonicalize("   # indented\nlet x = 1;\n");
        let b = canonicalize("let x = 1;\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn semicolon_drift_is_distinct() {
        let a = canonicalize("1 + 1;");
        let b = canonicalize("1 + 1");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn unparseable_falls_back_to_collapsed_text() {
        let c = canonicalize("@ not @ rust at all");
        assert!(c.unparsed);
        assert_eq!(c.text, "@ not @ rust at all");
        assert!(c.tokens > 0);
    }

    #[test]
    fn empty_body_is_empty() {
        let c = canonicalize("");
        assert_eq!(c.text, "");
        assert_eq!(c.tokens, 0);
    }
}
