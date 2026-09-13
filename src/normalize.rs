//! Canonical form of a doctest body.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use quote::ToTokens;
/// Canonical form of a doctest body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Canonical {
    /// Canonical text, a deterministic token-stream string, or the
    /// collapsed text fallback.
    pub(crate) text: String,
    /// True when no `syn` parse succeeded and the text fallback was used.
    pub(crate) unparsed: bool,
    /// Token count for `min-tokens` filtering.
    pub(crate) tokens: usize,
}

/// Compute the canonical form of a doctest body.
#[must_use]
pub(crate) fn canonicalize(code: &str) -> Canonical {
    // rustdoc drops any line whose first non-whitespace character is `#`.
    let body: String = code
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if let Ok(mut file) = syn::parse_str::<syn::File>(&body) {
        crate::alpha::normalize_file(&mut file);
        return from_stream(file.to_token_stream());
    }
    if let Ok(mut block) = syn::parse_str::<syn::Block>(&format!("{{\n{body}\n}}")) {
        crate::alpha::normalize_block(&mut block);
        return from_stream(block.to_token_stream());
    }
    if let Ok(mut expr) = syn::parse_str::<syn::Expr>(&body) {
        crate::alpha::normalize_expr(&mut expr);
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

    #[test]
    fn alpha_renames_let_bindings() {
        let a = canonicalize("let pino = 1;\npino + 1\n");
        let b = canonicalize("let abete = 1;\nabete + 1\n");
        assert_eq!(a.text, b.text);
        assert!(!a.unparsed);
    }

    #[test]
    fn alpha_renames_struct_shorthand_bindings() {
        let a = canonicalize("let P { pino } = make();\npino\n");
        let b = canonicalize("let P { pino: abete } = make();\nabete\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_closure_params() {
        let a = canonicalize("let f = |pino| pino + 1;\nf(2);\n");
        let b = canonicalize("let f = |abete| abete + 1;\nf(2);\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_free_idents_stay_distinct() {
        let a = canonicalize("foo();\n");
        let b = canonicalize("bar();\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_shadowing_resolves_to_nearest_binder() {
        let a = canonicalize("let x = 1;\n{ let x = 3; x }\nx\n");
        let b = canonicalize("let y = 1;\n{ let y = 3; y }\ny\n");
        assert_eq!(a.text, b.text);
        // The inner use resolves to the inner binder and the trailing
        // use to the outer one, so both canonical names appear.
        assert!(a.text.contains("_dejadoc_0"));
        assert!(a.text.contains("_dejadoc_1"));
        let c = canonicalize("let y = 2;\n{ let y = 1; y }\ny\n");
        assert_ne!(a.text, c.text);
    }

    #[test]
    fn alpha_renames_generics_and_lifetimes() {
        let a = canonicalize("fn f<'a, T>(x: &'a T) -> T { x.clone() }\n");
        let b = canonicalize("fn g<'b, U>(y: &'b U) -> U { y.clone() }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_types_and_self() {
        let a = canonicalize(
            "struct Pino { n: u8 }\nimpl Pino {\n    fn take(&self) -> u8 { self.n }\n}\n",
        );
        let b = canonicalize(
            "struct Abete { n: u8 }\nimpl Abete {\n    fn take(&self) -> u8 { self.n }\n}\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_field_names_stay_distinct() {
        let a = canonicalize("struct P { pino: u8 }\n");
        let b = canonicalize("struct P { abete: u8 }\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_macro_token_uses() {
        let a = canonicalize("let pino = 1;\nvec![pino]\n");
        let b = canonicalize("let abete = 1;\nvec![abete]\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_use_aliases() {
        let a = canonicalize("use std::collections::BTreeMap as Pino;\nPino::new();\n");
        let b = canonicalize("use std::collections::BTreeMap as Abete;\nAbete::new();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_does_not_touch_unparsed_fallback() {
        let a = canonicalize("let pino = @\n");
        let b = canonicalize("let abete = @\n");
        assert!(a.unparsed);
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_tuple_pattern() {
        let a = canonicalize("let (pino, qno) = pair;\npino + qno\n");
        let b = canonicalize("let (abete, ebano) = pair;\nabete + ebano\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_slice_pattern() {
        let a = canonicalize("let [pino, qno] = items;\npino + qno\n");
        let b = canonicalize("let [abete, ebano] = items;\nabete + ebano\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_tuple_struct_pattern() {
        let a = canonicalize("let Pair(pino, qno) = make();\npino + qno\n");
        let b = canonicalize("let Pair(abete, ebano) = make();\nabete + ebano\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_or_pattern() {
        let a = canonicalize("match it {\n    pino | qno => pino + qno,\n    _ => 0,\n}\n");
        let b = canonicalize("match it {\n    abete | ebano => abete + ebano,\n    _ => 0,\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_reference_pattern() {
        let a = canonicalize("let &pino = cell;\npino\n");
        let b = canonicalize("let &abete = cell;\nabete\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_paren_pattern() {
        let a = canonicalize("let (pino) = value;\npino\n");
        let b = canonicalize("let (abete) = value;\nabete\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_type_pattern() {
        let a = canonicalize("let pino: u8 = 1;\npino\n");
        let b = canonicalize("let abete: u8 = 1;\nabete\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_guard_pattern() {
        let a = canonicalize("match it {\n    pino if pino > 0 => pino,\n    _ => 0,\n}\n");
        let b = canonicalize("match it {\n    abete if abete > 0 => abete,\n    _ => 0,\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_scope_end_releases_inner_bindings() {
        let a = canonicalize("let pino = 1;\n{ let pino = 2; pino }\npino\n");
        assert_eq!(a.text.matches("_dejadoc_0").count(), 2);
        assert_eq!(a.text.matches("_dejadoc_1").count(), 2);
    }

    #[test]
    fn alpha_macro_token_content_preserved() {
        let a = canonicalize("let pino = 1;\nvec![pino]\n");
        let b = canonicalize("let pino = 1;\nvec![pino + 1]\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_normalize_expr_stage() {
        let mut expr = match syn::parse_str::<syn::Expr>("|pino| pino + 1") {
            Ok(expr) => expr,
            Err(err) => panic!("parse closure: {err}"),
        };
        crate::alpha::normalize_expr(&mut expr);
        let text = quote::quote!(#expr).to_string();
        assert!(text.contains("_dejadoc_0"));
        assert!(!text.contains("pino"));
    }

    #[test]
    fn alpha_renames_impl_method_locals() {
        let a = canonicalize(
            "struct Pino { n: u8 }\nimpl Pino {\n    fn take(&self, pino: u8) -> u8 { pino + self.n }\n}\n",
        );
        let b = canonicalize(
            "struct Abete { n: u8 }\nimpl Abete {\n    fn take(&self, abete: u8) -> u8 { abete + self.n }\n}\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_trait_method_locals() {
        let a = canonicalize("trait Pino {\n    fn f(&self, pino: u8) -> u8 { pino }\n}\n");
        let b = canonicalize("trait Abete {\n    fn f(&self, abete: u8) -> u8 { abete }\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_enum() {
        let a = canonicalize("enum Pino { Leaf, Branch(u8) }\n");
        let b = canonicalize("enum Abete { Leaf, Branch(u8) }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_union() {
        let a = canonicalize("union Pino { n: u8 }\n");
        let b = canonicalize("union Abete { n: u8 }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_type_alias() {
        let a = canonicalize("type Pino = u8;\n");
        let b = canonicalize("type Abete = u8;\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_trait() {
        let a = canonicalize("trait Pino {}\n");
        let b = canonicalize("trait Abete {}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_const() {
        let a = canonicalize("const PINO: u8 = 1;\nPINO + 1\n");
        let b = canonicalize("const ABETE: u8 = 1;\nABETE + 1\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_local_static() {
        let a = canonicalize("static PINO: u8 = 1;\nPINO + 1\n");
        let b = canonicalize("static ABETE: u8 = 1;\nABETE + 1\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_macro_definition() {
        let a = canonicalize("macro_rules! pino { () => { 1 } }\npino!()\n");
        let b = canonicalize("macro_rules! abete { () => { 1 } }\nabete!()\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_match_arm_locals() {
        let a = canonicalize("match it {\n    pino => pino + 1,\n    _ => 0,\n}\n");
        let b = canonicalize("match it {\n    abete => abete + 1,\n    _ => 0,\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_for_loop_locals() {
        let a = canonicalize("for pino in items {\n    use_it(pino)\n}\n");
        let b = canonicalize("for abete in items {\n    use_it(abete)\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_if_let_locals() {
        let a = canonicalize("if let Some(pino) = pick() {\n    use_it(pino)\n}\n");
        let b = canonicalize("if let Some(abete) = pick() {\n    use_it(abete)\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_while_let_locals() {
        let a = canonicalize("while let Some(pino) = next() {\n    use_it(pino)\n}\n");
        let b = canonicalize("while let Some(abete) = next() {\n    use_it(abete)\n}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_attribute_value_locals_renamed() {
        // A leading `#[...]` line is dropped as a rustdoc hidden line, so
        // the attribute sits inline to reach the parser.
        let a = canonicalize("let pino = 1; #[attr = pino] fn f() {}");
        let b = canonicalize("let abete = 1; #[attr = abete] fn f() {}");
        assert_eq!(a.text, b.text);
    }
    #[test]
    fn alpha_self_alias_collapses_to_local_type() {
        let a = canonicalize(
            "struct Pino { n: u8 }\nimpl Pino {\n    fn f(self) -> Self { self }\n}\n",
        );
        let b = canonicalize(
            "struct Pino { n: u8 }\nimpl Pino {\n    fn f(self) -> Pino { self }\n}\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_self_alias_requires_plain_self_type() {
        let c = canonicalize(
            "struct Abete { }\nstruct Pino { n: u8 }\nimpl Wrap for <Pino as Inner>::Abete {\n    fn f(self) { let s: Self = Self; }\n}\n",
        );
        assert!(c.text.contains("Self"));
    }
}
