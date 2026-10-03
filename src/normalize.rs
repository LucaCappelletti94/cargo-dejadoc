//! Canonical form of a doctest body.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use syn::ext::IdentExt;
use syn::visit_mut::VisitMut;
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
    let unhidden: String = code.lines().map(map_line).collect::<Vec<_>>().join("\n");
    let stripped = without_crate_attrs(&unhidden);
    let body = stripped.as_ref();
    let Some(mut file) = within_caps(body).then(|| parse_as_crate(body)).flatten() else {
        let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
        return Canonical {
            tokens: text.split_whitespace().count(),
            text,
            unparsed: true,
        };
    };
    IncludeDepth.visit_file_mut(&mut file);
    from_stream(syn_canon::canonicalize(file))
}

/// Deepest bracket and generic nesting a body may reach before it hashes as
/// text, past it the recursive parse and visits risk the end of the stack.
pub(crate) const MAX_NESTING: usize = 64;

/// Most leaf tokens a body may hold before it hashes as text, bounding the
/// length of operator, method and closure chains.
pub(crate) const MAX_TOKENS: usize = 16_384;

/// Whether `body` stays within `MAX_NESTING` and `MAX_TOKENS`, walked without
/// recursion. An open `<` counts as nesting until its `>`, a `;`, a `{…}`
/// block or the end of its group, so comparisons do not add up. A body that
/// does not tokenize is within the caps and fails to parse later.
fn within_caps(body: &str) -> bool {
    use proc_macro2::{Delimiter, TokenTree};

    let Ok(stream) = body.parse::<proc_macro2::TokenStream>() else {
        return true;
    };
    // One entry per open group, its remaining trees and its open `<` count.
    let mut stack = alloc::vec![(stream.into_iter(), 0_usize)];
    let (mut depth, mut tokens) = (0_usize, 0_usize);
    while let Some((trees, angles)) = stack.last_mut() {
        let Some(tree) = trees.next() else {
            depth -= *angles;
            stack.pop();
            depth = depth.saturating_sub(1);
            continue;
        };
        match tree {
            TokenTree::Group(group) => {
                if group.delimiter() == Delimiter::Brace {
                    depth -= *angles;
                    *angles = 0;
                }
                let inner = group.stream();
                drop(group);
                stack.push((inner.into_iter(), 0));
                depth += 1;
            }
            TokenTree::Punct(punct) => {
                tokens += 1;
                match punct.as_char() {
                    '<' => {
                        *angles += 1;
                        depth += 1;
                    }
                    '>' if *angles > 0 => {
                        *angles -= 1;
                        depth -= 1;
                    }
                    ';' => {
                        depth -= *angles;
                        *angles = 0;
                    }
                    _ => {}
                }
            }
            TokenTree::Ident(_) | TokenTree::Literal(_) => tokens += 1,
        }
        if depth > MAX_NESTING || tokens > MAX_TOKENS {
            return false;
        }
    }
    true
}

/// A body's leading `#![…]` attributes, which rustdoc lifts onto the
/// generated crate, then the rest of its tokens.
struct Lead {
    attrs: Vec<syn::Attribute>,
    rest: proc_macro2::TokenStream,
}

impl syn::parse::Parse for Lead {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            attrs: input.call(syn::Attribute::parse_inner)?,
            rest: input.parse()?,
        })
    }
}

/// The body without its leading crate attributes.
fn without_crate_attrs(body: &str) -> Cow<'_, str> {
    match syn::parse_str::<Lead>(body) {
        Ok(lead) if !lead.attrs.is_empty() => Cow::Owned(lead.rest.to_string()),
        _ => Cow::Borrowed(body),
    }
}

/// Parse `body` the way rustdoc compiles it, wrapped in `fn main` when
/// it declares none, with the bare `extern crate` lines rustdoc would
/// add itself dropped.
fn parse_as_crate(body: &str) -> Option<syn::File> {
    let is_main = |item: &syn::Item| matches!(item, syn::Item::Fn(f) if f.sig.ident == "main");
    let bare_extern = |item: &syn::Item| matches!(item, syn::Item::ExternCrate(e) if e.rename.is_none() && e.attrs.is_empty());
    let mut file = match syn::parse_str::<syn::File>(body) {
        Ok(file) if file.items.iter().any(is_main) => file,
        _ => {
            let mut block: syn::Block = syn::parse_str(&format!("{{\n{body}\n}}")).ok()?;
            block
                .stmts
                .retain(|stmt| !matches!(stmt, syn::Stmt::Item(item) if bare_extern(item)));
            let output = take_result_tail(&mut block).map(|err| quote::quote!(-> Result<(), #err>));
            syn::parse2(quote::quote!(fn main() #output #block)).ok()?
        }
    };
    file.items.retain(|item| !bare_extern(item));
    Some(file)
}

/// The error type of a trailing `Ok::<(), E>(())`, which becomes `Ok(())`.
fn take_result_tail(block: &mut syn::Block) -> Option<syn::Type> {
    let syn::Stmt::Expr(syn::Expr::Call(call), None) = block.stmts.last_mut()? else {
        return None;
    };
    let unit_arg = call.args.len() == 1
        && matches!(call.args.first(), Some(syn::Expr::Tuple(t)) if t.elems.is_empty());
    let syn::Expr::Path(func) = call.func.as_mut() else {
        return None;
    };
    if !unit_arg
        || func.qself.is_some()
        || func.path.leading_colon.is_some()
        || func.path.segments.len() != 1
    {
        return None;
    }
    let segment = func.path.segments.first_mut()?;
    let syn::PathArguments::AngleBracketed(args) = &mut segment.arguments else {
        return None;
    };
    let unit_ok = segment.ident == "Ok"
        && args.args.len() == 2
        && matches!(&args.args[0], syn::GenericArgument::Type(syn::Type::Tuple(t)) if t.elems.is_empty())
        && matches!(&args.args[1], syn::GenericArgument::Type(_));
    if !unit_ok {
        return None;
    }
    let syn::GenericArgument::Type(err) = args.args.pop()? else {
        return None;
    };
    segment.arguments = syn::PathArguments::None;
    Some(err)
}

/// rustdoc's hidden-line rule per body line. A `##` line shows as a `#`
/// line. A `# ` prefix (space required) hides the line but keeps its
/// trimmed remainder. A bare `#` hides to an empty line. Everything
/// else, including `#text` without the space, is kept verbatim.
fn map_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.starts_with("##") {
        line.replacen("##", "#", 1)
    } else if let Some(stripped) = trimmed.strip_prefix("# ") {
        stripped.to_string()
    } else if trimmed == "#" {
        String::new()
    } else {
        line.to_string()
    }
}

/// Strip the leading `../` of `include`-style macro paths, which rustdoc resolves from the package root.
struct IncludeDepth;

impl VisitMut for IncludeDepth {
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        let include = mac
            .path
            .segments
            .last()
            .is_some_and(|s| is_include(&s.ident));
        mac.tokens = flatten_stream(core::mem::take(&mut mac.tokens), include);
    }

    fn visit_meta_list_mut(&mut self, list: &mut syn::MetaList) {
        list.tokens = flatten_stream(core::mem::take(&mut list.tokens), false);
    }
}

fn is_include(ident: &proc_macro2::Ident) -> bool {
    let name = ident.unraw().to_string();
    name == "include" || name.starts_with("include_")
}

/// Flatten include paths in one token list, `strip` set by an `include` macro name and kept only by its `!`.
fn flatten_stream(stream: proc_macro2::TokenStream, strip: bool) -> proc_macro2::TokenStream {
    use proc_macro2::{Group, TokenTree};

    let mut strip = strip;
    stream
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Ident(id) => {
                strip = is_include(&id);
                TokenTree::Ident(id)
            }
            TokenTree::Punct(p) => {
                strip = strip && p.as_char() == '!';
                TokenTree::Punct(p)
            }
            TokenTree::Group(group) => {
                let mut rebuilt =
                    Group::new(group.delimiter(), flatten_stream(group.stream(), strip));
                rebuilt.set_span(group.span());
                strip = false;
                TokenTree::Group(rebuilt)
            }
            TokenTree::Literal(lit) => {
                let lit = if strip {
                    flat_literal(&lit).unwrap_or(lit)
                } else {
                    lit
                };
                strip = false;
                TokenTree::Literal(lit)
            }
        })
        .collect()
}

/// The literal with leading `../` components removed, `None` unless it is
/// a string literal (plain, raw, or byte) whose text starts with depth.
/// Prefix and hash runs are preserved, escape sequences are untouched.
fn flat_literal(lit: &proc_macro2::Literal) -> Option<proc_macro2::Literal> {
    let s = lit.to_string();
    let open = s.find('"')?;
    let prefix = &s[..open];
    let hashes = prefix.trim_start_matches(|c: char| c != '#');
    let close = s.len().checked_sub(1 + hashes.len())?;
    let inner = s.get(open + 1..close)?;
    if !inner.starts_with("../") {
        return None;
    }
    let mut stripped = inner;
    while let Some(rest) = stripped.strip_prefix("../") {
        stripped = rest;
    }
    format!("{prefix}\"{stripped}\"{hashes}").parse().ok()
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

    /// Whether `code` hashes as text, canonicalized on the stack `group` uses.
    fn hashes_as_text(code: String) -> bool {
        std::thread::Builder::new()
            .stack_size(crate::GROUP_STACK)
            .spawn(move || canonicalize(&code).unparsed)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nesting_past_the_cap_hashes_as_text() {
        let nested = |n: usize| format!("let x = {}1{};", "(".repeat(n), ")".repeat(n));
        assert!(!hashes_as_text(nested(MAX_NESTING)));
        assert!(hashes_as_text(nested(MAX_NESTING + 1)));
    }

    #[test]
    fn generic_nesting_counts_toward_the_cap() {
        let nested = |n: usize| format!("let x: {}u8{} = v;", "Vec<".repeat(n), ">".repeat(n));
        assert!(!hashes_as_text(nested(MAX_NESTING)));
        assert!(hashes_as_text(nested(MAX_NESTING + 1)));
    }

    #[test]
    fn a_long_body_past_the_token_cap_hashes_as_text() {
        let statements = "x;".repeat(MAX_TOKENS / 2);
        assert!(!hashes_as_text(statements.clone()));
        assert!(hashes_as_text(format!("{statements}x")));
    }

    #[test]
    fn comparisons_do_not_add_up_across_statements_blocks_or_groups() {
        let calls = format!("let v = [{}];", "f(a < b), ".repeat(MAX_NESTING + 1));
        let semicolons = "let c = a < b;\n".repeat(MAX_NESTING + 1);
        let blocks = "if a < b {}\n".repeat(MAX_NESTING + 1);
        for body in [calls, semicolons, blocks] {
            assert!(!hashes_as_text(body.clone()), "{body}");
        }
    }

    #[test]
    fn closed_generics_do_not_add_up() {
        let tuple = format!("let x: ({}) = v;", "Vec<u8>, ".repeat(MAX_NESTING + 1));
        assert!(!hashes_as_text(tuple));
    }

    #[test]
    fn include_path_depth_is_not_a_difference() {
        let near = r#"# include!("../doctest_setup.rs");
# fn main() { run().unwrap(); }"#;
        let far = r#"# include!("../../doctest_setup.rs");
# fn main() { run().unwrap(); }"#;
        assert_eq!(canonicalize(near).text, canonicalize(far).text);
    }

    #[test]
    fn a_deeper_path_with_a_different_file_is_not_merged() {
        let a = r#"# include!("../a/x.rs");
# fn main() {}"#;
        let b = r#"# include!("../../../a/y.rs");
# fn main() {}"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn a_printed_relative_path_is_its_own_content() {
        let a = r#"fn main() { println!("../status"); }"#;
        let b = r#"fn main() { println!("status"); }"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn include_depth_folds_inside_other_tokens() {
        let pairs = [
            (
                "let v = vec![include_str!(\"../a.md\")];",
                "let v = vec![include_str!(\"a.md\")];",
            ),
            (
                "#[cfg_attr(docsrs, doc = include_str!(\"../README.md\"))]\nfn f() {}",
                "#[cfg_attr(docsrs, doc = include_str!(\"README.md\"))]\nfn f() {}",
            ),
        ];
        for (deep, flat) in pairs {
            assert_eq!(canonicalize(deep).text, canonicalize(flat).text, "{deep}");
        }
    }

    #[test]
    fn a_nested_macro_path_is_its_own_content() {
        let a = canonicalize("let v = vec![format!(\"../x\")];");
        let b = canonicalize("let v = vec![format!(\"x\")];");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn include_str_depth_is_not_a_difference() {
        let a = r#"let readme = include_str!("../README.md");"#;
        let b = r#"let readme = include_str!("README.md");"#;
        assert_eq!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn include_bytes_depth_is_not_a_difference() {
        let a = r#"let icon = include_bytes!("../../assets/icon.png");"#;
        let b = r#"let icon = include_bytes!("assets/icon.png");"#;
        assert_eq!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn raw_include_paths_keep_their_hashes() {
        let a = r##"let spec = include_str!(r#"../SPEC.md"#);"##;
        let b = r##"let spec = include_str!(r#"SPEC.md"#);"##;
        assert_eq!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn a_raw_spelled_include_is_still_an_include() {
        let near = r#"# r#include!("../doctest_setup.rs");
# fn main() {}"#;
        let far = r#"# r#include!("../../doctest_setup.rs");
# fn main() {}"#;
        assert_eq!(canonicalize(near).text, canonicalize(far).text);
    }

    #[test]
    fn a_variable_named_includes_holds_content() {
        let a = r#"let includes = "../parts.cfg";"#;
        let b = r#"let includes = "parts.cfg";"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn a_variable_named_include_holds_content() {
        let a = r#"let include = "../parts.cfg";"#;
        let b = r#"let include = "parts.cfg";"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn a_method_named_includes_holds_content() {
        let a = r#"let hit = config.includes("../parts.cfg");"#;
        let b = r#"let hit = config.includes("parts.cfg");"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn escaped_quotes_do_not_end_a_literal() {
        let a = "# include!(\"../a\\\"b.rs\");\nfn main() {}";
        let b = "# include!(\"a\\\"b.rs\");\nfn main() {}";
        assert_eq!(canonicalize(a).text, canonicalize(b).text);
    }

    #[test]
    fn unparsed_bodies_keep_their_text_verbatim() {
        let a = canonicalize("include!(\"../x.rs\",");
        let b = canonicalize("include!(\"x.rs\",");
        assert!(a.unparsed && b.unparsed);
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn comment_and_whitespace_drift_collapses() {
        let a = canonicalize("fn main() { }\n// a comment");
        let b = canonicalize("fn main(){}  /* other */  \n");
        assert_eq!(a.text, b.text);
        assert_eq!(a.tokens, b.tokens);
    }

    #[test]
    fn raw_string_hash_counts_agree() {
        assert_eq!(
            canonicalize(r##"r#"a"#"##).text,
            canonicalize(r###"r##"a"##"###).text,
        );
    }

    #[test]
    fn hidden_lint_attr_merges() {
        let a = canonicalize("# #[allow(unused)]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
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
        assert_eq!(c.tokens, 5);
    }

    #[test]
    fn hidden_line_keeps_its_content() {
        let a = canonicalize("# use foo::bar;\nbar();\n");
        let b = canonicalize("use foo::bar;\nbar();\n");
        assert_eq!(a.text, b.text);
        assert!(!a.unparsed);
    }

    #[test]
    fn implicit_main_three_spellings_are_one_test() {
        let stmt = canonicalize("let x = 1;\nassert_eq!(x, 1);");
        let explicit = canonicalize("fn main() {\n    let x = 1;\n    assert_eq!(x, 1);\n}");
        let hidden = canonicalize("# fn main() {\nlet x = 1;\nassert_eq!(x, 1);\n# }");
        assert_eq!(stmt.text, explicit.text);
        assert_eq!(explicit.text, hidden.text);
    }

    #[test]
    fn implicit_main_nested_scopes_match_explicit() {
        let a = canonicalize("let x = 1;\n{\n    let y = x + 1;\n    assert_eq!(y, 2);\n}");
        let b = canonicalize(
            "fn main() {\n    let x = 1;\n    {\n        let y = x + 1;\n        assert_eq!(y, 2);\n    }\n}",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn implicit_main_item_body_wrapped() {
        let items = canonicalize("struct P;\nlet p = P;");
        let wrapped = canonicalize("fn main() { struct P; let p = P; }");
        assert_eq!(items.text, wrapped.text);
    }

    #[test]
    fn implicit_main_after_other_items_is_not_double_wrapped() {
        let body = "fn helper() -> u8 { 1 }\nfn main() { let _x = helper(); }";
        let double = canonicalize(&alloc::format!("fn main() {{\n{body}\n}}"));
        let direct = canonicalize(body);
        assert_ne!(double.text, direct.text);
    }

    #[test]
    fn bare_expression_equals_wrapped_main() {
        let bare = canonicalize("1 + 1");
        let wrapped = canonicalize("fn main() { 1 + 1 }");
        assert_eq!(bare.text, wrapped.text);
    }

    #[test]
    fn hidden_extern_crate_equals_no_extern_crate() {
        let with_ec = canonicalize("# extern crate foo;\nfoo::run();");
        let without_ec = canonicalize("foo::run();");
        assert_eq!(with_ec.text, without_ec.text);
    }

    #[test]
    fn an_attributed_extern_crate_stays() {
        let a = canonicalize("#[macro_use]\nextern crate foo;\nbar!();");
        let b = canonicalize("bar!();");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn aliased_extern_crate_stays_distinct() {
        let aliased = canonicalize("extern crate foo as bar;\nbar::run();");
        let plain = canonicalize("bar::run();");
        assert_ne!(aliased.text, plain.text);
    }

    #[test]
    fn multiple_anonymous_extern_crates_dropped() {
        let a = canonicalize(
            "# extern crate foo;\n# extern crate bar;\nextern crate baz as qux;\nfoo::a();\nbar::b();\nqux::c();",
        );
        let b = canonicalize("extern crate baz as qux;\nfoo::a();\nbar::b();\nqux::c();");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn ok_tail_equals_result_main() {
        let tail = canonicalize("let x = f()?;\nOk::<(), E>(())");
        let explicit =
            canonicalize("fn main() -> Result<(), E> {\n    let x = f()?;\n    Ok(())\n}");
        assert_eq!(tail.text, explicit.text);
    }

    #[test]
    fn an_ok_tail_with_a_lifetime_argument_keeps_it() {
        let a = canonicalize("let x = f()?;\nOk::<(), 'a>(())");
        let b = canonicalize("let x = f()?;\nOk::<()>(())");
        assert_ne!(a.text, b.text);
        assert!(a.text.contains('\''));
    }

    #[test]
    fn only_the_exact_ok_tail_becomes_a_result_main() {
        for (tail, call) in [
            ("Ok::<(), E>(1)", "Ok(1)"),
            ("Ok::<(), E>((), 1)", "Ok((), 1)"),
            ("Err::<(), E>(())", "Err(())"),
            ("<T>::Ok::<(), E>(())", "<T>::Ok(())"),
            ("::Ok::<(), E>(())", "::Ok(())"),
            ("m::Ok::<(), E>(())", "m::Ok(())"),
        ] {
            let bare = canonicalize(&format!("let x = f()?;\n{tail}"));
            let explicit = canonicalize(&format!(
                "fn main() -> Result<(), E> {{ let x = f()?; {call} }}"
            ));
            assert_ne!(bare.text, explicit.text, "{tail}");
        }
        let unit_err = canonicalize("fn main() -> Result<(), ()> {\n    Ok(())\n}");
        assert_ne!(canonicalize("Ok::<()>(())").text, unit_err.text);
    }

    #[test]
    fn ok_tail_complex_body_matches_result_main() {
        let tail = canonicalize("let x = g()?;\nlet y = x + 1;\nOk::<(), MyError>(())");
        let explicit = canonicalize(
            "fn main() -> Result<(), MyError> {\n    let x = g()?;\n    let y = x + 1;\n    Ok(())\n}",
        );
        assert_eq!(tail.text, explicit.text);
    }

    #[test]
    fn ok_tail_different_error_types_stay_distinct() {
        let a = canonicalize("Ok::<(), std::io::Error>(())");
        let b = canonicalize("Ok::<(), String>(())");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn ok_without_turbofish_does_not_match_result_tail() {
        let plain = canonicalize("Ok(())");
        let result = canonicalize("fn main() -> Result<(), ()> { Ok(()) }");
        assert_ne!(plain.text, result.text);
    }

    #[test]
    fn hash_without_space_is_kept() {
        let a = canonicalize("#let x = 1;\nx\n");
        assert!(a.text.contains("#let"));
        let b = canonicalize("# let x = 1;\nx\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn double_hash_shows_as_single_hash() {
        let a = canonicalize("## setup\nx\n");
        assert!(a.text.contains("# setup"));
    }

    #[test]
    fn bare_hash_line_becomes_empty() {
        let a = canonicalize("#\nlet x = 1;\n");
        let b = canonicalize("let x = 1;\n");
        assert_eq!(a.text, b.text);
        assert!(!a.unparsed);
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
        assert!(!c.unparsed);
        assert!(c.tokens > 0);
    }

    #[test]
    fn leading_inner_attributes_are_lifted() {
        let a =
            canonicalize("#![allow(unused)]\n#![feature(x)]\n# use x::Y;\nlet pino = Y::new();\n");
        let b = canonicalize("# use x::Y;\nlet abete = Y::new();\n");
        assert!(!a.unparsed);
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
    fn include_depth_folds_on_byte_path_literals() {
        // flat_literal documents byte support. This pins flattening for a
        // `b"…"`, distinct_byte_include_paths_stay_distinct must hold
        // whichever way the contract call goes.
        let near = canonicalize("let i = include_image!(b\"../icon.png\");");
        let here = canonicalize("let i = include_image!(b\"icon.png\");");
        assert_eq!(near.text, here.text);
    }

    #[test]
    fn include_depth_folds_on_raw_byte_path_literals() {
        let near = canonicalize("let i = include_image!(br\"../icon.png\");");
        let here = canonicalize("let i = include_image!(br\"icon.png\");");
        assert_eq!(near.text, here.text);
    }

    #[test]
    fn include_depth_folds_on_c_string_path_literals() {
        let near = canonicalize("let i = include_image!(c\"../icon\");");
        let here = canonicalize("let i = include_image!(c\"icon\");");
        assert_eq!(near.text, here.text);
    }

    #[test]
    fn distinct_byte_include_paths_stay_distinct() {
        // The buggy flat_literal drops the last path char of a non-raw
        // prefixed literal, which merged `../x.png` with `../x.pnq`.
        let a = canonicalize("let i = include_image!(b\"../x.png\");");
        let b = canonicalize("let i = include_image!(b\"../x.pnq\");");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn distinct_c_string_include_paths_stay_distinct() {
        let a = canonicalize("let i = include_image!(c\"../a\");");
        let b = canonicalize("let i = include_image!(c\"../b\");");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn indented_hidden_line_is_trimmed() {
        let a = canonicalize("   # let x = 1;\n   x\n");
        let b = canonicalize("let x = 1;\nx\n");
        assert_eq!(a.text, b.text);
    }
}
