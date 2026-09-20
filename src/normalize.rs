//! Canonical form of a doctest body.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use quote::ToTokens;
use syn::ext::IdentExt;
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
    let Some(mut file) = parse_as_crate(body) else {
        let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
        return Canonical {
            tokens: text.split_whitespace().count(),
            text,
            unparsed: true,
        };
    };
    crate::drift::normalize_file(&mut file);
    crate::alpha::normalize_file(&mut file);
    let stream = flatten_include_depth(file.to_token_stream());
    let stream = crate::drift::strip_trailing_commas(stream);
    let stream = crate::drift::canonical_literals(stream);
    from_stream(stream)
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

/// Strips the leading `../` components of path literals passed to
/// `include`-style macros (`include!`, `include_str!`, egui's
/// `include_image!`). The depth is an artifact of the file spelling the
/// include: rustdoc compiles a doctest from the package root, so either
/// spelling resolves to the same file, while a `println!` or `File::open`
/// path is runtime content and every component of it matters.
fn flatten_include_depth(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    flatten_stream(stream, false)
}

/// Walks one token list. `strip` is set by a macro name (`include` or
/// `include_…`) and kept only by the following `!`, so strip mode is
/// entered exactly by macro call syntax, never by a method or a binding
/// named `includes`. In include mode the delimiter's string literals
/// lose their leading `../` components. Any group or literal ends it.
fn flatten_stream(stream: proc_macro2::TokenStream, strip: bool) -> proc_macro2::TokenStream {
    let mut out = proc_macro2::TokenStream::new();
    let mut strip = strip;
    for tree in stream {
        match tree {
            proc_macro2::TokenTree::Ident(id) => {
                let name = id.unraw().to_string();
                strip = name == "include" || name.starts_with("include_");
                out.extend(core::iter::once(proc_macro2::TokenTree::Ident(id)));
            }
            proc_macro2::TokenTree::Punct(p) => {
                strip = strip && p.as_char() == '!';
                out.extend(core::iter::once(proc_macro2::TokenTree::Punct(p)));
            }
            proc_macro2::TokenTree::Group(group) => {
                let inner = flatten_stream(group.stream(), strip);
                strip = false;
                let mut out_group = proc_macro2::Group::new(group.delimiter(), inner);
                out_group.set_span(group.span());
                out.extend(core::iter::once(proc_macro2::TokenTree::Group(out_group)));
            }
            proc_macro2::TokenTree::Literal(lit) => {
                let kept = if strip {
                    flat_literal(&lit).map_or(proc_macro2::TokenTree::Literal(lit), |f| {
                        proc_macro2::TokenTree::from(f)
                    })
                } else {
                    proc_macro2::TokenTree::Literal(lit)
                };
                strip = false;
                out.extend(core::iter::once(kept));
            }
        }
    }
    out
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
    fn a_runtime_relative_path_is_its_own_content() {
        let a = r#"let f = File::open("../data.csv").unwrap();"#;
        let b = r#"let f = File::open("data.csv").unwrap();"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
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
    fn macro_call_bracket_and_paren_merge() {
        let a = canonicalize("vec![1, 2]");
        let b = canonicalize("vec!(1, 2)");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_call_all_three_delimiters_merge() {
        let bracket = canonicalize("vec![1, 2];\nf();");
        let paren = canonicalize("vec!(1, 2);\nf();");
        let brace = canonicalize("vec! {1, 2}\nf();");
        assert_eq!(bracket.text, paren.text);
        assert_eq!(paren.text, brace.text);
    }

    #[test]
    fn statement_macro_brace_merges_with_paren() {
        let a = canonicalize("println! {\"hello\"}\nf();");
        let b = canonicalize("println!(\"hello\");\nf();");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_tail_macro_without_a_semicolon_stays() {
        let tail = canonicalize("macro_rules! five { () => { 5 } }\nfn f() -> i32 { five! {} }");
        let discarded =
            canonicalize("macro_rules! five { () => { 5 } }\nfn f() -> i32 { five!(); }");
        assert_ne!(tail.text, discarded.text);
    }

    #[test]
    fn a_non_tail_macro_statement_merges_either_delimiter() {
        let a = canonicalize("m! {1}\nf();");
        let b = canonicalize("m!(1);\nf();");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_in_type_position_delimiter_merges() {
        let a = canonicalize("type T = my_type![u32];");
        let b = canonicalize("type T = my_type!(u32);");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_in_pattern_position_delimiter_merges() {
        let a = canonicalize("match 42u8 { my_pat![42] => 1u8, _ => 2u8 }");
        let b = canonicalize("match 42u8 { my_pat!(42) => 1u8, _ => 2u8 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_in_an_impl_item_delimiter_merges() {
        let a = canonicalize("struct S;\nimpl S { my_items! {1} }");
        let b = canonicalize("struct S;\nimpl S { my_items!(1); }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_in_a_trait_item_delimiter_merges() {
        let a = canonicalize("trait T { my_items! {1} }");
        let b = canonicalize("trait T { my_items!(1); }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn item_level_macro_without_ident_merges() {
        let a = canonicalize("fn main() {}\nfoo! { x }");
        let b = canonicalize("fn main() {}\nfoo!(x);");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn nested_macro_outer_delimiter_merges() {
        let a = canonicalize("outer![inner!(x)]");
        let b = canonicalize("outer!(inner!(x))");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_argument_bracket_group_survives() {
        let a = canonicalize("foo![arr[0]]");
        let b = canonicalize("foo!(arr[0])");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_rules_inner_delimiter_stays_distinct() {
        let a = canonicalize("fn main() {}\nmacro_rules! m { ([$a:expr]) => { $a }; }");
        let b = canonicalize("fn main() {}\nmacro_rules! m { (($a:expr)) => { $a }; }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn macro_calls_with_different_args_stay_distinct() {
        let a = canonicalize("foo!(1)");
        let b = canonicalize("foo!(2)");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn macro_call_vs_plain_call_stays_distinct() {
        let a = canonicalize("foo!(x)");
        let b = canonicalize("foo(x)");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn macro_rules_body_delimiter_stays_distinct() {
        let a = canonicalize("fn main() {}\nmacro_rules! m { ($a:expr) => { [$a] }; }");
        let b = canonicalize("fn main() {}\nmacro_rules! m { ($a:expr) => { ($a) }; }");
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
    fn integer_underscore_and_plain_agree() {
        assert_eq!(canonicalize("1_000").text, canonicalize("1000").text);
    }

    #[test]
    fn integer_hex_and_decimal_agree() {
        assert_eq!(canonicalize("0x1F").text, canonicalize("31").text);
    }

    #[test]
    fn a_stringified_literal_keeps_its_spelling() {
        let a = canonicalize("assert_eq!(stringify!(0x10), \"0x10\");");
        let b = canonicalize("assert_eq!(stringify!(16), \"0x10\");");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn a_matcher_literal_keeps_its_spelling() {
        let a = canonicalize("macro_rules! m { (0x10) => { 1 } }\nm!(0x10);");
        let b = canonicalize("macro_rules! m { (16) => { 1 } }\nm!(16);");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn integer_binary_and_decimal_agree() {
        assert_eq!(canonicalize("0b1010").text, canonicalize("10").text);
    }

    #[test]
    fn integer_octal_and_decimal_agree() {
        assert_eq!(canonicalize("0o17").text, canonicalize("15").text);
    }

    #[test]
    fn integer_suffixed_across_radices_agree() {
        assert_eq!(canonicalize("0x1u8").text, canonicalize("1u8").text);
    }

    #[test]
    fn float_trailing_dot_and_dot_zero_agree() {
        assert_eq!(canonicalize("1.").text, canonicalize("1.0").text);
    }

    #[test]
    fn float_trailing_zeros_agree() {
        assert_eq!(canonicalize("1.50").text, canonicalize("1.500").text);
        assert_eq!(canonicalize("1.50").text, canonicalize("1.5").text);
    }

    #[test]
    fn a_float_canonicalizes_to_its_text_form() {
        assert_eq!(canonicalize("1_000.50").text, "fn _dejadoc_0 () { 1000.5 }");
        assert_eq!(canonicalize("1.0E+03").text, "fn _dejadoc_0 () { 1.0e3 }");
    }

    #[test]
    fn float_exponent_form_stays_distinct() {
        assert_ne!(canonicalize("1e3").text, canonicalize("1000.0").text);
        assert_eq!(canonicalize("1.0E+03").text, canonicalize("1.0e3").text);
    }

    #[test]
    fn char_unicode_escape_and_literal_agree() {
        assert_eq!(canonicalize(r"'\u{41}'").text, canonicalize("'A'").text);
    }

    #[test]
    fn exponent_sign_and_padding_agree() {
        assert_eq!(canonicalize("1.0e-03").text, canonicalize("1.0e-3").text);
        assert_eq!(canonicalize("1.0e+3").text, canonicalize("1.0e3").text);
        assert_ne!(canonicalize("1.0e-3").text, canonicalize("1.0e3").text);
    }

    #[test]
    fn byte_and_c_literals_agree_with_their_escapes() {
        assert_eq!(canonicalize(r"b'\x41'").text, canonicalize("b'A'").text);
        assert_eq!(canonicalize(r#"br"a""#).text, canonicalize(r#"b"a""#).text);
        assert_eq!(canonicalize(r#"cr"a""#).text, canonicalize(r#"c"a""#).text);
        assert_ne!(canonicalize(r#"b"a""#).text, canonicalize(r#"b"b""#).text);
    }

    #[test]
    fn char_longer_unicode_escape_and_literal_agree() {
        assert_eq!(canonicalize(r"'\u{0041}'").text, canonicalize("'A'").text);
    }

    #[test]
    fn raw_string_and_escaped_string_agree() {
        assert_eq!(
            canonicalize(r#"r"hello""#).text,
            canonicalize(r#""hello""#).text
        );
    }

    #[test]
    fn raw_string_hash_counts_agree() {
        assert_eq!(
            canonicalize(r##"r#"a"#"##).text,
            canonicalize(r###"r##"a"##"###).text,
        );
    }

    #[test]
    fn macro_argument_literals_stay_opaque() {
        let a = canonicalize("assert_eq!(1_000, 1000);");
        let b = canonicalize("assert_eq!(1000, 1000);");
        assert_ne!(a.text, b.text);
        let c = canonicalize("vec![vec![0x01, 0x02]]");
        let d = canonicalize("vec![vec![1, 2]]");
        assert_ne!(c.text, d.text);
    }

    #[test]
    fn string_literal_in_attribute_value_stays_opaque() {
        let a = canonicalize(r#"#[my_attr(label = r"same")] fn f() {}"#);
        let b = canonicalize(r#"#[my_attr(label = "same")] fn f() {}"#);
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_attribute_argument_literal_stays_opaque() {
        let attr = canonicalize("#[my_attr(label = 0x10)]\nfn f() {}");
        assert_ne!(
            attr.text,
            canonicalize("#[my_attr(label = 16)]\nfn f() {}").text
        );
        let call = canonicalize("my_attr!(label = 0x10);");
        assert_ne!(call.text, canonicalize("my_attr!(label = 16);").text);
    }

    #[test]
    fn different_integers_stay_distinct() {
        assert_ne!(canonicalize("1").text, canonicalize("2").text);
    }

    #[test]
    fn different_hex_values_stay_distinct() {
        assert_ne!(canonicalize("0x10").text, canonicalize("0x20").text);
    }

    #[test]
    fn different_strings_stay_distinct() {
        assert_ne!(
            canonicalize(r#""hello""#).text,
            canonicalize(r#""world""#).text
        );
    }

    #[test]
    fn different_chars_stay_distinct() {
        assert_ne!(canonicalize("'a'").text, canonicalize("'b'").text);
    }

    #[test]
    fn suffixed_and_unsuffixed_integer_stay_distinct() {
        assert_ne!(canonicalize("1u8").text, canonicalize("1").text);
    }

    #[test]
    fn different_float_values_stay_distinct() {
        assert_ne!(canonicalize("1.0").text, canonicalize("2.0").text);
    }

    #[test]
    fn trailing_comma_struct_literal_merges() {
        let a = canonicalize("let _p = P { x: 1, y: 2 };");
        let b = canonicalize("let _p = P { x: 1, y: 2, };");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn trailing_comma_call_merges() {
        let a = canonicalize("foo(1, 2);");
        let b = canonicalize("foo(1, 2,);");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn one_tuple_after_a_keyword_keeps_its_comma() {
        let a = canonicalize("struct S;\ntrait T {}\nimpl T for (S,) {}");
        let b = canonicalize("struct S;\ntrait T {}\nimpl T for (S) {}");
        assert_ne!(a.text, b.text);
        let c = canonicalize("fn f<T>() where (T,): Copy {}");
        let d = canonicalize("fn f<T>() where (T): Copy {}");
        assert_ne!(c.text, d.text);
    }

    #[test]
    fn trailing_comma_vec_macro_merges() {
        let a = canonicalize("vec![1, 2]");
        let b = canonicalize("vec![1, 2,]");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn trailing_comma_nested_struct_in_call_merges() {
        let a = canonicalize("foo(P { x: 1, y: 2 })");
        let b = canonicalize("foo(P { x: 1, y: 2, },)");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn one_tuple_stays_distinct() {
        let a = canonicalize("let t = (1,);");
        let b = canonicalize("let t = (1);");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn two_tuple_trailing_comma_merges() {
        let a = canonicalize("let t = (1, 2,);");
        let b = canonicalize("let t = (1, 2);");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn match_arm_block_single_expr_merges() {
        let a = canonicalize("match v { Some(x) => { foo(x) }, None => 0, }");
        let b = canonicalize("match v { Some(x) => foo(x), None => 0, }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn labelled_or_attributed_arm_block_is_kept() {
        let plain = canonicalize("match v { Some(x) => { foo(x) }, None => 0, }");
        let labelled = canonicalize("match v { Some(x) => 'a: { foo(x) }, None => 0, }");
        let attributed = canonicalize("match v { Some(x) => #[allow(x)] { foo(x) }, None => 0, }");
        assert_ne!(labelled.text, plain.text);
        assert_ne!(attributed.text, plain.text);
    }

    #[test]
    fn a_statement_arm_block_keeps_its_statement() {
        let a = canonicalize("match v { Some(x) => { foo(x); } None => {} }");
        let b = canonicalize("match v { Some(x) => { bar(x); } None => {} }");
        assert_ne!(a.text, b.text);
        let c = canonicalize("match v { Some(x) => { m!(x); } None => {} }");
        let d = canonicalize("match v { Some(x) => { n!(x); } None => {} }");
        assert_ne!(c.text, d.text);
    }

    #[test]
    fn match_arm_block_with_let_stays_distinct() {
        let a = canonicalize("match v { Some(x) => { let y = x; foo(y) }, None => 0, }");
        let b = canonicalize("match v { Some(x) => foo(x), None => 0, }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn match_arm_block_with_semicolon_stays_distinct() {
        let a = canonicalize("match v { Some(x) => { foo(x); }, None => 0, }");
        let b = canonicalize("match v { Some(x) => foo(x), None => 0, }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn doc_comment_on_local_fn_merges() {
        let a = canonicalize("/// Doc comment.\nfn f() -> u8 { 1 }");
        let b = canonicalize("fn f() -> u8 { 1 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn doc_comments_on_every_item_kind_merge() {
        let body = |doc: &str| {
            [
                "fn main() {}",
                "const C: u8 = 1;",
                "enum E { A }",
                "#[macro_use]\nextern crate foo;",
                "extern \"C\" { fn cf(); }",
                "impl S {}",
                "m! {}",
                "mod md {}",
                "static ST: u8 = 1;",
                "struct S;",
                "trait T {}",
                "trait TA = T;",
                "type Ty = u8;",
                "union U { n: u8 }",
                "use a::b;",
            ]
            .iter()
            .fold(String::new(), |mut out, item| {
                out.push_str(doc);
                out.push_str(item);
                out.push('\n');
                out
            })
        };
        assert_eq!(
            canonicalize(&body("/// Doc\n")).text,
            canonicalize(&body("")).text
        );
    }

    #[test]
    fn doc_comments_on_statements_merge() {
        let a = canonicalize("/// Doc\nlet x = 1;\n/// Doc\nm!(x);\n");
        let b = canonicalize("let x = 1;\nm!(x);\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn doc_comments_on_impl_and_trait_items_merge() {
        let body = |doc: &str| {
            format!(
                "trait T {{ {doc}const C: u8; {doc}fn f(&self); {doc}type A; {doc}m!(); }}\n\
                 struct S;\nimpl T for S {{ {doc}const C: u8 = 1; {doc}fn f(&self) {{}} {doc}type A = u8; {doc}m!(); }}\n"
            )
        };
        assert_eq!(
            canonicalize(&body("#[doc = \"d\"] ")).text,
            canonicalize(&body("")).text
        );
    }

    #[test]
    fn doc_comment_on_variant_merges() {
        let a = canonicalize("enum E {\n    /// Doc\n    A,\n}");
        let b = canonicalize("enum E { A }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn lint_attr_on_fn_merges() {
        let a = canonicalize("#[allow(unused)]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn deny_and_forbid_stay() {
        let plain = canonicalize("fn f() { let x = 1; }");
        assert_ne!(
            canonicalize("#[deny(unused)]\nfn f() { let x = 1; }").text,
            plain.text
        );
        assert_ne!(
            canonicalize("#[forbid(unused)]\nfn f() { let x = 1; }").text,
            plain.text
        );
        assert_eq!(
            canonicalize("#[warn(unused)]\nfn f() { let x = 1; }").text,
            plain.text
        );
    }

    #[test]
    fn lint_attr_on_struct_field_merges() {
        let a = canonicalize("struct S {\n    #[allow(dead_code)]\n    x: u8,\n}");
        let b = canonicalize("struct S { x: u8 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn lint_attr_on_stmt_merges() {
        let a = canonicalize("#[allow(unused_variables)]\nlet x = 1;");
        let b = canonicalize("let x = 1;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn lint_attr_on_impl_item_merges() {
        let a = canonicalize(
            "struct S;\nimpl S {\n    #[allow(clippy::unused_self)]\n    fn f(&self) {}\n}",
        );
        let b = canonicalize("struct S;\nimpl S { fn f(&self) {} }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn two_lint_attrs_on_one_item_merges() {
        let a = canonicalize("#[allow(unused)]\n#[warn(dead_code)]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn expect_attr_merges() {
        let a = canonicalize(
            r#"#[expect(unused, reason = "demo")]
fn f() {}"#,
        );
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn hidden_lint_attr_merges() {
        let a = canonicalize("# #[allow(unused)]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn cfg_attr_stays_distinct() {
        let a = canonicalize("#[cfg(unix)]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn derive_attr_stays_distinct() {
        let a = canonicalize("#[derive(Debug)]\nstruct S;");
        let b = canonicalize("struct S;");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn repr_attr_stays_distinct() {
        let a = canonicalize("#[repr(C)]\nstruct S { x: u8 }");
        let b = canonicalize("struct S { x: u8 }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn unknown_attr_stays_distinct() {
        let a = canonicalize("#[my_attr]\nfn f() {}");
        let b = canonicalize("fn f() {}");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn struct_declared_after_use_merges() {
        let a = canonicalize("let _p = P;\nstruct P;");
        let b = canonicalize("struct P;\nlet _p = P;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn every_hoistable_item_kind_moves() {
        const ITEMS: &[&str] = &[
            "const C: u8 = 1;",
            "enum E { A }",
            "extern crate foo as bar;",
            "fn g() {}",
            "extern \"C\" { fn cf(); }",
            "impl S {}",
            "mod md {}",
            "static ST: u8 = 1;",
            "struct S;",
            "trait T {}",
            "trait TA = T;",
            "type Ty = u8;",
            "union U { n: u8 }",
            "use a::b;",
        ];
        for item in ITEMS {
            let front = canonicalize(&format!("#[allow(unused)]\n{item}\nlet _x = 1;\n"));
            let back = canonicalize(&format!("let _x = 1;\n#[allow(unused)]\n{item}\n"));
            assert_eq!(front.text, back.text, "{item}");
        }
    }

    #[test]
    fn an_item_with_a_live_attribute_keeps_its_place() {
        const ITEMS: &[&str] = &[
            "const C: u8 = 1;",
            "enum E { A }",
            "extern crate foo as bar;",
            "fn g() {}",
            "extern \"C\" { fn cf(); }",
            "impl S {}",
            "m! {}",
            "mod md {}",
            "static ST: u8 = 1;",
            "struct S;",
            "trait T {}",
            "trait TA = T;",
            "type Ty = u8;",
            "union U { n: u8 }",
        ];
        for item in ITEMS {
            let front = canonicalize(&format!("#[cfg(unix)]\n{item}\nlet _x = 1;\n"));
            let back = canonicalize(&format!("let _x = 1;\n#[cfg(unix)]\n{item}\n"));
            assert_ne!(front.text, back.text, "{item}");
        }
    }

    #[test]
    fn fn_declared_after_call_merges() {
        let a = canonicalize("f();\nfn f() {}");
        let b = canonicalize("fn f() {}\nf();");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn two_items_reversed_merges() {
        let a = canonicalize("struct B;\nstruct A;");
        let b = canonicalize("struct A;\nstruct B;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn use_and_item_either_order_merges() {
        let a = canonicalize("use core::fmt;\nstruct S;");
        let b = canonicalize("struct S;\nuse core::fmt;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn macro_rules_not_hoisted_past() {
        let a = canonicalize("m!();\nmacro_rules! m { () => {} }");
        let b = canonicalize("macro_rules! m { () => {} }\nm!();");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn doc_comment_on_struct_field_merges() {
        let a = canonicalize("struct S {\n    /// Field doc.\n    x: u8,\n}");
        let b = canonicalize("struct S {\n    x: u8,\n}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn inner_doc_in_fn_body_merges() {
        let a = canonicalize("fn f() -> u8 {\n    //! Inner doc.\n    1\n}");
        let b = canonicalize("fn f() -> u8 {\n    1\n}");
        if !a.unparsed && !b.unparsed {
            assert_eq!(a.text, b.text);
        }
    }

    #[test]
    fn use_single_vs_group_merges() {
        let a = canonicalize("use a::b;\nb();\n");
        let b = canonicalize("use a::{b};\nb();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn use_two_lines_vs_tree_merges() {
        let a = canonicalize("use a::b;\nuse a::c;\nb(); c();\n");
        let b = canonicalize("use a::{b, c};\nb(); c();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn use_reversed_order_merges() {
        let a = canonicalize("use a::c;\nuse a::b;\nc(); b();\n");
        let b = canonicalize("use a::b;\nuse a::c;\nc(); b();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn use_glob_in_group_merges() {
        let a = canonicalize("use a::*;\nfoo();\n");
        let b = canonicalize("use a::{*};\nfoo();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn use_self_leaf_is_the_module() {
        let a = canonicalize("use a::{self};\nfoo();\n");
        let b = canonicalize("use a;\nfoo();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn pub_use_stays_distinct() {
        let a = canonicalize("pub use a::b;\n");
        let b = canonicalize("use a::b;\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn different_use_leaf_sets_stay_distinct() {
        let a = canonicalize("use a::b;\nuse a::c;\n");
        let b = canonicalize("use a::b;\nuse a::d;\n");
        assert_ne!(a.text, b.text);
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
    fn fn_with_different_name_stays_distinct_from_bare() {
        let with_run = canonicalize("fn run() {}");
        let bare = canonicalize("()");
        assert_ne!(with_run.text, bare.text);
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
    fn indented_hidden_line_is_trimmed() {
        let a = canonicalize("   # let x = 1;\n   x\n");
        let b = canonicalize("let x = 1;\nx\n");
        assert_eq!(a.text, b.text);
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
    fn semicolon_drift_is_distinct() {
        let a = canonicalize("1 + 1;");
        let b = canonicalize("1 + 1");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn paren_expr_folds() {
        let a = canonicalize("(1 + 2)");
        let b = canonicalize("1 + 2");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn double_paren_folds() {
        let a = canonicalize("((1 + 2))");
        let b = canonicalize("1 + 2");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn paren_precedence_preserved() {
        let a = canonicalize("(1 + 2) * 3");
        let b = canonicalize("1 + 2 * 3");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn expr_paren_vs_one_tuple_stays_distinct() {
        let a = canonicalize("let _ = (1,);");
        let b = canonicalize("let _ = (1);");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn paren_in_pattern_folds() {
        let a = canonicalize("let (x) = 1;");
        let b = canonicalize("let x = 1;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn one_tuple_pattern_stays_distinct() {
        let a = canonicalize("let (x,) = (1,);");
        let b = canonicalize("let (x) = (1,);");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn paren_in_type_folds() {
        let a = canonicalize("fn f(x: (u8)) -> (u8) { x }");
        let b = canonicalize("fn f(x: u8) -> u8 { x }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn one_tuple_type_stays_distinct() {
        let a = canonicalize("fn f(_: (u8,)) {}");
        let b = canonicalize("fn f(_: (u8)) {}");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn closure_body_block_unwraps() {
        let a = canonicalize("|x| { x + 1 }");
        let b = canonicalize("|x| x + 1");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn nested_closure_block_unwraps() {
        let a = canonicalize("|| { || { 1 + 2 } }");
        let b = canonicalize("|| || 1 + 2");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn closure_two_stmts_block_stays() {
        let a = canonicalize("|x| { let y = x; y }");
        let b = canonicalize("|x| x");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn return_tail_folds() {
        let a = canonicalize("fn f() { return 1; }");
        let b = canonicalize("fn f() { 1 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn return_tail_in_closure_block_folds() {
        let a = canonicalize("|x| { return x + 1; }");
        let b = canonicalize("|x| x + 1");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn early_return_stays() {
        let a = canonicalize("fn f() { return 1; g(); }");
        let b = canonicalize("fn f() { 1; g(); }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn bare_return_stays() {
        let a = canonicalize("fn f() { return; }");
        let b = canonicalize("fn f() {}");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn unit_return_type_folds_fn() {
        let a = canonicalize("fn f() -> () {}");
        let b = canonicalize("fn f() {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn closure_unit_return_type_stays() {
        let a = canonicalize("|| -> () { 1 }");
        let b = canonicalize("|| 1");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn non_unit_return_type_stays() {
        let a = canonicalize("fn f() -> u8 { 1 }");
        let b = canonicalize("fn f() { 1 }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn double_semicolon_at_end_folds() {
        let a = canonicalize("fn f() { g();; }");
        let b = canonicalize("fn f() { g(); }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn double_semicolon_in_middle_folds() {
        let a = canonicalize("fn f() { g();; h(); }");
        let b = canonicalize("fn f() { g(); h(); }");
        assert_eq!(a.text, b.text);
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
    fn alpha_fn_forward_ref_collapses() {
        let a = canonicalize("fn main() { helper(); }\nfn helper() {}\n");
        let b = canonicalize("fn main() { aux(); }\nfn aux() {}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_struct_forward_ref_collapses() {
        let a = canonicalize("fn main() { drop(Pino { n: 1 }); }\nstruct Pino { n: u8 }\n");
        let b = canonicalize("fn main() { drop(Abete { n: 1 }); }\nstruct Abete { n: u8 }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_impl_constructor_forward_ref_collapses() {
        let a = canonicalize(
            "fn main() { let _ = Pino::new(); }\nstruct Pino { n: u8 }\nimpl Pino { fn new() -> Self { Pino { n: 0 } } }\n",
        );
        let b = canonicalize(
            "fn main() { let _ = Abete::new(); }\nstruct Abete { n: u8 }\nimpl Abete { fn new() -> Self { Abete { n: 0 } } }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_const_forward_ref_collapses() {
        let a = canonicalize("fn main() { let _ = FOO; }\nconst FOO: u8 = 1;\n");
        let b = canonicalize("fn main() { let _ = BAR; }\nconst BAR: u8 = 1;\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_every_item_kind_forward_ref_collapses() {
        let body = |s: &str, e: &str, u: &str, t: &str, tr: &str| {
            format!(
                "fn main() {{ let _ = {s}; let _ = {e}::A; let _ = {u} {{ n: 1 }}; let _: {t} = 1; }}\n\
                 fn g<X: {tr}>() {{}}\n\
                 static {s}: u8 = 1;\nenum {e} {{ A }}\nunion {u} {{ n: u8 }}\ntype {t} = u8;\ntrait {tr} {{}}\n"
            )
        };
        let a = canonicalize(&body("S1", "E1", "U1", "T1", "R1"));
        let b = canonicalize(&body("S2", "E2", "U2", "T2", "R2"));
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_nested_fn_forward_ref_collapses() {
        let a = canonicalize("fn main() { helper(); fn helper() {} }\n");
        let b = canonicalize("fn main() { aux(); fn aux() {} }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_mutual_recursion_collapses() {
        let a = canonicalize(
            "fn even(n: u32) -> bool { if n == 0 { true } else { odd(n - 1) } }\nfn odd(n: u32) -> bool { if n == 0 { false } else { even(n - 1) } }\n",
        );
        let b = canonicalize(
            "fn par(n: u32) -> bool { if n == 0 { true } else { impar(n - 1) } }\nfn impar(n: u32) -> bool { if n == 0 { false } else { par(n - 1) } }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_forward_ref_free_name_stays_distinct() {
        let a = canonicalize("fn main() { foreign(); }\n");
        let b = canonicalize("fn main() { other_fn(); }\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_inline_mod_items_renamed() {
        let a = canonicalize("mod m { pub fn f() {} }\nfn main() { m::f(); }\n");
        let b = canonicalize("mod n { pub fn g() {} }\nfn main() { n::g(); }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_module_used_above_its_definition() {
        let a = canonicalize("fn main() { m::f(); }\nmod m { pub fn f() {} }\n");
        let b = canonicalize("fn main() { n::g(); }\nmod n { pub fn g() {} }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_nested_module_used_above_its_definition() {
        let a = canonicalize(
            "fn main() { m::inner::f(); }\nmod m { pub mod inner { pub fn f() {} } }\n",
        );
        let b =
            canonicalize("fn main() { n::deep::g(); }\nmod n { pub mod deep { pub fn g() {} } }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_inline_mod_partial_call_collapses() {
        let a = canonicalize("mod m { pub fn f() {} pub fn h() {} }\nfn main() { m::f(); }\n");
        let b = canonicalize("mod n { pub fn g() {} pub fn k() {} }\nfn main() { n::g(); }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_inline_mod_distinct_structures_differ() {
        let a = canonicalize(
            "mod m { pub fn f() {} }\nmod n { pub fn g() {} }\nfn main() { m::f(); n::g(); }\n",
        );
        let b =
            canonicalize("mod r { pub fn p() {} pub fn q() {} }\nfn main() { r::p(); r::q(); }\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_generics_and_lifetimes() {
        let a = canonicalize("fn f<'a, T>(x: &'a T) -> T { x.clone() }\n");
        let b = canonicalize("fn g<'b, U>(y: &'b U) -> U { y.clone() }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_closure_higher_ranked_lifetimes() {
        let a = canonicalize("let f = for<'a> |x: &'a i32| x;");
        let b = canonicalize("let f = for<'b> |x: &'b i32| x;");
        assert_eq!(a.text, b.text);
        let c = canonicalize("let f = for<'a> |x: &'static i32| x;");
        assert_ne!(a.text, c.text);
    }

    #[test]
    fn alpha_where_clause_two_params_lifetime_bound() {
        let a = canonicalize(
            "fn f<'a, T, U>(x: &'a T, y: U) -> T where T: Clone, U: Into<T> + 'a { x.clone() }\n",
        );
        let b = canonicalize(
            "fn g<'b, V, W>(p: &'b V, q: W) -> V where V: Clone, W: Into<V> + 'b { p.clone() }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_where_clause_on_impl_block() {
        let a = canonicalize(
            "struct Pino;\nimpl<T: Clone> Pino where T: Default { fn f(&self, _: T) {} }\n",
        );
        let b = canonicalize(
            "struct Abete;\nimpl<U: Clone> Abete where U: Default { fn f(&self, _: U) {} }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_where_clause_on_struct() {
        let a = canonicalize("struct Wrap<T>(T) where T: Clone;\n");
        let b = canonicalize("struct Pack<U>(U) where U: Clone;\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_where_free_trait_stays_distinct() {
        let a = canonicalize("fn f<T>() where T: Clone {}\n");
        let b = canonicalize("fn f<T>() where T: Default {}\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_impl_local_trait_collapses() {
        let a = canonicalize("trait Pino {}\nimpl Pino for u8 {}\n");
        let b = canonicalize("trait Abete {}\nimpl Abete for u8 {}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_impl_local_trait_with_generic() {
        let a = canonicalize("trait Pino<T> {}\nstruct S;\nimpl<T> Pino<T> for S {}\n");
        let b = canonicalize("trait Abete<U> {}\nstruct S;\nimpl<U> Abete<U> for S {}\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_closure_typed_two_params_return_type() {
        let a = canonicalize("struct Pino;\nlet f = |x: Pino, y: Pino| -> Pino { x };\n");
        let b = canonicalize("struct Abete;\nlet f = |p: Abete, q: Abete| -> Abete { p };\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_let_chain_three_lets_bool_middle() {
        let a = canonicalize(concat!(
            "if let Some(pino) = a() && cond",
            " && let Some(qno) = b() && let Some(rno) = c()",
            " { use_it(pino, qno, rno) }\n",
        ));
        let b = canonicalize(concat!(
            "if let Some(abete) = a() && cond",
            " && let Some(ebano) = b() && let Some(faggio) = c()",
            " { use_it(abete, ebano, faggio) }\n",
        ));
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_let_chain_shadow_outer_else_uses_outer() {
        let a = canonicalize(
            "let pino = 0;\nif let Some(pino) = get() { use_it(pino) } else { pino }\n",
        );
        let b = canonicalize(
            "let abete = 0;\nif let Some(abete) = get() { use_it(abete) } else { abete }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_nested_labelled_loops_inner_break_outer() {
        let a = canonicalize("'outer: loop { 'inner: loop { break 'outer; } break 'outer; }\n");
        let b = canonicalize("'x: loop { 'y: loop { break 'x; } break 'x; }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_labelled_block_expression() {
        let a = canonicalize("let v = 'pino: { break 'pino 1; };\n");
        let b = canonicalize("let v = 'abete: { break 'abete 1; };\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_unlabelled_break_stays_distinct() {
        let a = canonicalize("loop { break; }\n");
        let b = canonicalize("'x: loop { break 'x; }\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_struct_expr_shorthand_one_local_one_free() {
        let a = canonicalize("let pino = 1;\nP { pino, free_field }\n");
        let b = canonicalize("let abete = 1;\nP { pino: abete, free_field }\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_struct_expr_shorthand_nested() {
        let a = canonicalize("let pino = 1;\nlet qno = P { pino };\nOuter { inner: qno }\n");
        let b = canonicalize(
            "let abete = 1;\nlet ebano = P { pino: abete };\nOuter { inner: ebano }\n",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_struct_expr_shorthand_free_stays_distinct() {
        let a = canonicalize("P { pino }\n");
        let b = canonicalize("P { abete }\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_self_alias_generic_impl() {
        let a = canonicalize(
            "struct Pino<T>(T);\nimpl<T> Pino<T> { fn n(t: T) -> Self { Self(t) } }\n",
        );
        let b = canonicalize(
            "struct Abete<T>(T);\nimpl<T> Abete<T> { fn n(t: T) -> Abete<T> { Abete(t) } }\n",
        );
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
    fn leading_inner_attributes_are_lifted() {
        let a =
            canonicalize("#![allow(unused)]\n#![feature(x)]\n# use x::Y;\nlet pino = Y::new();\n");
        let b = canonicalize("# use x::Y;\nlet abete = Y::new();\n");
        assert!(!a.unparsed);
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_use_of_different_items_stays_distinct() {
        let a = canonicalize("use pkg::pino;\npino();\n");
        let b = canonicalize("use pkg::abete;\nabete();\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_use_name_equals_its_aliased_form() {
        let a = canonicalize("use pkg::{pino, sub::*};\npino();\n");
        let b = canonicalize("use pkg::{pino as abete, sub::*};\nabete();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_use_group_keeps_each_item() {
        let a = canonicalize("use pkg::{pino, qno};\npino(qno);\n");
        let b = canonicalize("use pkg::{pino, ebano};\npino(ebano);\n");
        assert_ne!(a.text, b.text);
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
        assert_eq!(a.text.matches("_dejadoc_1").count(), 2);
        assert_eq!(a.text.matches("_dejadoc_2").count(), 2);
    }

    #[test]
    fn alpha_macro_token_content_preserved() {
        let a = canonicalize("let pino = 1;\nvec![pino]\n");
        let b = canonicalize("let pino = 1;\nvec![pino + 1]\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_closure_in_macro_arg_renamed() {
        let a = canonicalize("assert!(v.iter().map(|x| x > 0).any(|x| x))\n");
        let b = canonicalize("assert!(v.iter().map(|y| y > 0).any(|y| y))\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_match_binder_in_macro_arg_renamed() {
        let a = canonicalize("assert!(match v { x => x > 0 })\n");
        let b = canonicalize("assert!(match v { y => y > 0 })\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_block_let_in_macro_arg_renamed() {
        let a = canonicalize("vec![{ let x = 1; x + 1 }]\n");
        let b = canonicalize("vec![{ let y = 1; y + 1 }]\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_nested_local_macro_in_macro_arg_renamed() {
        let a = canonicalize("macro_rules! pino { () => { 1 } }\nassert_eq!(pino!(), 1)\n");
        let b = canonicalize("macro_rules! abete { () => { 1 } }\nassert_eq!(abete!(), 1)\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_vec_repeat_with_bound_count() {
        let a = canonicalize("let n = 5;\nvec![0; n]\n");
        let b = canonicalize("let m = 5;\nvec![0; m]\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_binder_inside_the_repeat_element_is_renamed() {
        let a = canonicalize("vec![{ let x = 1; x }; 2]\n");
        let b = canonicalize("vec![{ let y = 1; y }; 2]\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_longer_semicolon_list_keeps_the_token_walk() {
        let a = canonicalize("m!({ let x = 1; x }; 1; 2)\n");
        let b = canonicalize("m!({ let y = 1; y }; 1; 2)\n");
        assert_ne!(a.text, b.text);
        let c = canonicalize("m!({ let x = 1; x }; 1;)\n");
        let d = canonicalize("m!({ let y = 1; y }; 1;)\n");
        assert_ne!(c.text, d.text);
    }

    #[test]
    fn alpha_field_access_not_renamed_for_unrelated_local() {
        let a = canonicalize("let x = 1;\nprintln!(\"{}\", s.x)\n");
        let b = canonicalize("let y = 1;\nprintln!(\"{}\", s.x)\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_inline_format_arg_debug_renamed() {
        let a = canonicalize("let x = v;\nprintln!(\"{x:?}\")\n");
        let b = canonicalize("let y = v;\nprintln!(\"{y:?}\")\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_escaped_braces_in_format_not_renamed() {
        let a = canonicalize("let x = 1;\nprintln!(\"{{x}}\")\n");
        let b = canonicalize("let y = 1;\nprintln!(\"{{y}}\")\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn a_placeholder_between_escapes_is_renamed() {
        let a = canonicalize("let x = 1;\nprintln!(\"{{{x}}}\")\n");
        let b = canonicalize("let y = 1;\nprintln!(\"{{{y}}}\")\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_format_width_param_renamed() {
        let a = canonicalize("let x = 1;\nlet w = 5;\nprintln!(\"{x:>w$}\")\n");
        let b = canonicalize("let y = 1;\nlet z = 5;\nprintln!(\"{y:>z$}\")\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_positional_and_inline_format_args() {
        let a = canonicalize("let x = 1;\nprintln!(\"{} {x}\", x)\n");
        let b = canonicalize("let y = 1;\nprintln!(\"{} {y}\", y)\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_macro_name_in_fallback_path_renamed() {
        let a = canonicalize("macro_rules! pino { () => { 1 } }\nouter!(pino! if foo);\n");
        let b = canonicalize("macro_rules! abete { () => { 1 } }\nouter!(abete! if foo);\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn fallback_tokens_keep_their_content() {
        let a = canonicalize("outer!(pino! if foo);\n");
        let b = canonicalize("outer!(pino! if bar);\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn alpha_free_name_in_format_string_stays_distinct() {
        let a = canonicalize("println!(\"{free_name}\")\n");
        let b = canonicalize("println!(\"{other_free}\")\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn a_value_literal_is_not_a_format_string() {
        let a = canonicalize("let x = 1;\nprintln!(\"{}\", \"{x}\");\n");
        let b = canonicalize("let y = 1;\nprintln!(\"{}\", \"{y}\");\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn write_format_operand_is_the_second_argument() {
        let a = canonicalize("let x = 1;\nwrite!(out, \"{x}\").unwrap();\n");
        let b = canonicalize("let y = 1;\nwrite!(out, \"{y}\").unwrap();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn assert_eq_message_is_the_third_argument() {
        let a = canonicalize("let x = 1;\nassert_eq!(x, 1, \"{x}\");\n");
        let b = canonicalize("let y = 1;\nassert_eq!(y, 1, \"{y}\");\n");
        assert_eq!(a.text, b.text);
        let c = canonicalize("let x = 1;\nassert_eq!(\"{x}\", 1);\n");
        let d = canonicalize("let y = 1;\nassert_eq!(\"{y}\", 1);\n");
        assert_ne!(c.text, d.text);
    }

    #[test]
    fn an_unknown_macro_keeps_its_literals() {
        let a = canonicalize("let x = 1;\nmy_log!(\"{x}\");\n");
        let b = canonicalize("let y = 1;\nmy_log!(\"{y}\");\n");
        assert_ne!(a.text, b.text);
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
    fn alpha_renames_local_trait_alias() {
        let a = canonicalize("trait Pino = Clone;\nfn f<T: Pino>() {}\n");
        let b = canonicalize("trait Abete = Clone;\nfn f<T: Abete>() {}\n");
        assert_eq!(a.text, b.text);
        let c = canonicalize("trait Pino = Clone;\nfn f<T: Copy>() {}\n");
        assert_ne!(a.text, c.text);
        let forward_pino = canonicalize("fn f<T: Pino>() {}\ntrait Pino = Clone;\n");
        let forward_abete = canonicalize("fn f<T: Abete>() {}\ntrait Abete = Clone;\n");
        assert_eq!(forward_pino.text, forward_abete.text);
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
    fn alpha_keeps_unit_variant_patterns() {
        let a = canonicalize("match it {\n    None => 0,\n    Some(v) => v,\n}\n");
        let b = canonicalize("match it {\n    Empty => 0,\n    Some(v) => v,\n}\n");
        assert_ne!(a.text, b.text);
        let c = canonicalize("let Unit = make();\n");
        let d = canonicalize("let Other = make();\n");
        assert_ne!(c.text, d.text);
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
        // A leading `#[...]` line is kept, so the attribute sits inline
        // to reach the parser.
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

    #[test]
    fn a_return_in_a_let_block_is_not_the_fn_return_value() {
        // rustc returns 1 from the first body and 2 from the second.
        // return_tail_folds is sound only where the block tail is the fn body.
        let early = canonicalize("fn f() -> u8 { let _a = { return 1; }; 2 }");
        let value = canonicalize("fn f() -> u8 { let _a = { 1 }; 2 }");
        assert_ne!(early.text, value.text);
    }

    #[test]
    fn a_return_in_an_argument_block_is_not_the_fn_return_value() {
        let early = canonicalize("fn f() -> u8 { g({ return 1; }); 2 }");
        let value = canonicalize("fn f() -> u8 { g({ 1 }); 2 }");
        assert_ne!(early.text, value.text);
    }

    #[test]
    fn a_return_in_a_loop_body_is_not_a_tail_expression() {
        // The first body returns on the first pass, the second never
        // terminates.
        let early = canonicalize("fn f() -> u8 { while c { return 1; } 2 }");
        let looped = canonicalize("fn f() -> u8 { while c { 1 } 2 }");
        assert_ne!(early.text, looped.text);
    }

    #[test]
    fn a_return_in_a_tail_match_arm_folds() {
        // A tail match arm value is the fn result, `return 1;` and `1`
        // return the same value, verified equivalent with rustc.
        let early = canonicalize("fn f() -> u8 { match v { A => { return 1; }, _ => 2 } }");
        let value = canonicalize("fn f() -> u8 { match v { A => { 1 }, _ => 2 } }");
        assert_eq!(early.text, value.text);
    }

    #[test]
    fn a_return_in_a_non_tail_match_arm_is_not_the_discarded_value() {
        // The semicolon drops the match value while an early return still
        // escapes the fn, so that arm must not fold.
        let early = canonicalize("fn f() -> u8 { match v { A => { return 1; }, _ => {} }; 2 }");
        let value = canonicalize("fn f() -> u8 { match v { A => { 1 }, _ => {} }; 2 }");
        assert_ne!(early.text, value.text);
    }

    #[test]
    fn a_return_in_a_tail_unsafe_block_folds() {
        let a = canonicalize("fn f() -> u8 { unsafe { return 1; } }");
        let b = canonicalize("fn f() -> u8 { unsafe { 1 } }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_return_in_tail_if_else_branches_folds() {
        let a = canonicalize(
            "fn f(c: bool, d: bool) -> u8 { if c { return 1; } else if d { return 2; } else { return 3; } }",
        );
        let b =
            canonicalize("fn f(c: bool, d: bool) -> u8 { if c { 1 } else if d { 2 } else { 3 } }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_return_in_a_no_else_if_keeps_its_return() {
        // An `if` without an `else` discards its then value and rustc
        // rejects a valued return there (`E0317`), so this shape cannot
        // compile and must not fold.
        let a = canonicalize("fn f() -> u8 { if c { return 1; } }");
        let b = canonicalize("fn f() -> u8 { if c { 1 } }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_impl_fn_tail_return_folds() {
        let a = canonicalize("impl T for S { fn f(&self) -> u8 { return 1; } }");
        let b = canonicalize("impl T for S { fn f(&self) -> u8 { 1 } }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_trait_default_fn_tail_return_folds() {
        let a = canonicalize("trait T { fn f() -> u8 { return 1; } }");
        let b = canonicalize("trait T { fn f() -> u8 { 1 } }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_inert_attr_on_a_fn_pointer_parameter_merges() {
        // rustc, verified, accepts an attribute on a named fn pointer
        // parameter, the strip lives in `visit_named_arg_mut`.
        let a = canonicalize("let f: fn(#[expect(unused)] x: u8) = g;");
        let b = canonicalize("let f: fn(x: u8) = g;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_inert_attr_on_a_method_receiver_merges() {
        // rustc, verified, accepts an attribute on a method receiver.
        let a = canonicalize("impl S { fn m(#[expect(unused)] &self) -> u8 { 1 } }");
        let b = canonicalize("impl S { fn m(&self) -> u8 { 1 } }");
        assert_eq!(a.text, b.text);
    }
    #[test]
    fn a_single_expr_arm_block_unwraps_in_a_non_tail_match() {
        // The arm block unwrap lives in `visit_arm_mut`, not in the tail
        // fold chain, so a match whose value is discarded must unwrap too.
        let a = canonicalize("fn f() -> u8 { match v { A => { 1 }, _ => 2 }; 2 }");
        let b = canonicalize("fn f() -> u8 { match v { A => 1, _ => 2 }; 2 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_folded_return_expression_reenters_the_chain() {
        // Folding the tail `return` exposes the value as the new tail,
        // its own tail return folds too, rustc verified equivalent.
        let a = canonicalize("fn f() -> u8 { return { g(); return 2; }; }");
        let b = canonicalize("fn f() -> u8 { { g(); 2 } }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_higher_ranked_lifetime_does_not_shadow_a_loop_label() {
        // rustc verified, label `'a` and `for<'a>` are distinct namespaces,
        // the `break` must keep pointing at the label.
        let a = canonicalize(
            "fn f() -> u8 { 'a: loop { let g: for<'a> fn(&'a u8) -> &'a u8 = h; break 'a 1; } }",
        );
        let b = canonicalize(
            "fn f() -> u8 { 'zz: loop { let g: for<'q> fn(&'q u8) -> &'q u8 = h; break 'zz 1; } }",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_higher_ranked_dyn_bound_does_not_shadow_a_loop_label() {
        let a = canonicalize(
            "fn f() -> u8 { 'a: loop { let d: &dyn for<'a> Fn(&'a u8) = g; break 'a 1; } }",
        );
        let b = canonicalize(
            "fn f() -> u8 { 'zz: loop { let d: &dyn for<'q> Fn(&'q u8) = g; break 'zz 1; } }",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_tail_return_without_semicolon_folds() {
        let a = canonicalize("fn f() -> u8 { return 1 }");
        let b = canonicalize("fn f() -> u8 { 1 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_cfg_attr_on_a_tail_return_keeps_the_return() {
        // rustc, verified: with the cfg off the fn falls through to 2,
        // folding the return away would change the result.
        let a = canonicalize(
            "fn f() -> u8 { #[cfg(never_flag)] #[expect(unreachable_code)] return 1; 2 }",
        );
        let b = canonicalize("fn f() -> u8 { 1; 2 }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_inert_attr_on_a_tail_return_still_folds() {
        let a = canonicalize("fn f() -> u8 { #[expect(unreachable_code)] return 1; }");
        let b = canonicalize("fn f() -> u8 { 1 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn inert_attrs_on_closure_parameter_patterns_merge() {
        // An attribute on an untyped closure parameter lands on the pattern
        // and each kind reaching `strip_pat_inert_attrs` merges.
        let params = [
            "x",
            "&k",
            "(p, q)",
            "(Some(s))",
            "Some(r)",
            "[m, ..]",
            "P { x }",
            "_g",
            "_",
            "1 | 2",
            "None",
            "1",
            "1..=5",
            "mac!()",
        ];
        for param in params {
            let with = canonicalize(&format!("let c = |#[expect(unused)] {param}| 0;"));
            let without = canonicalize(&format!("let c = |{param}| 0;"));
            assert_eq!(with.text, without.text, "param pattern {param}");
        }
    }

    #[test]
    fn a_return_in_an_async_block_is_its_tail_value() {
        // rustc reads an async block's `return` as its tail, so this fold is
        // sound here as it is in fn and closure bodies.
        let a = canonicalize("let f = async { return 1; };");
        let b = canonicalize("let f = async { 1 };");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_inert_attr_on_a_match_arm_merges() {
        // rustc accepts attributes on arms and inert lint attributes carry no
        // program meaning, as expect_attr_merges pins for items.
        let a = canonicalize("match v { #[expect(unused_variables)] Some(_) => {}, None => {} }");
        let b = canonicalize("match v { Some(_) => {}, None => {} }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_cfg_attr_on_a_match_arm_stays_distinct() {
        let a = canonicalize("match v { #[cfg(unix)] Some(_) => {}, None => {} }");
        let b = canonicalize("match v { Some(_) => {}, None => {} }");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_inert_attr_on_a_fn_param_merges() {
        // rustc accepts `fn f(#[...] x: u8)` and the attribute is inert.
        let a = canonicalize("fn f(#[expect(unused_variables)] x: u8) { x }");
        let b = canonicalize("fn f(x: u8) { x }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_literal_in_a_local_macro_stays_opaque() {
        let a = canonicalize("m!(0x10);\nf();");
        let b = canonicalize("m!(16);\nf();");
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
    fn alpha_renames_let_chain_binder_in_a_match_guard() {
        // rustc accepts let-chains in arm guards and scopes the binder over
        // guard and arm body. skills/dejadoc/SKILL.md declares local names
        // never separate two bodies.
        let a = canonicalize("match w { Some(v) if let Some(z) = q && z == v => z, _ => 2 }");
        let b = canonicalize("match w { Some(v) if let Some(t) = q && t == v => t, _ => 2 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_higher_ranked_lifetime_in_a_dyn_bound() {
        // alpha_renames_closure_higher_ranked_lifetimes pins the closure
        // binder. SKILL.md makes no distinction for one in a type.
        let a = canonicalize("let f: &dyn for<'a> Fn(&'a u8) = g;");
        let b = canonicalize("let f: &dyn for<'b> Fn(&'b u8) = g;");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_higher_ranked_lifetime_in_a_where_bound() {
        let a = canonicalize("fn g<T>(x: &T) where T: for<'a> Tr<'a> {}");
        let b = canonicalize("fn g<T>(x: &T) where T: for<'b> Tr<'b> {}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_erases_fn_pointer_parameter_names() {
        // A fn-pointer argument name is decorative, rustc resolves nothing
        // through it, so every spelling names one type.
        let named = canonicalize("let f: fn(a: u8) = g;");
        let other = canonicalize("let f: fn(b: u8) = g;");
        let discard = canonicalize("let f: fn(_: u8) = g;");
        let bare = canonicalize("let f: fn(u8) = g;");
        assert_eq!(named.text, other.text);
        assert_eq!(named.text, discard.text);
        assert_eq!(named.text, bare.text);
    }
}
