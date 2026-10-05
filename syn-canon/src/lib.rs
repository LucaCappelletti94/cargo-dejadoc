#![doc = include_str!("../README.md")]
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod alpha;
mod drift;

use alloc::vec::Vec;
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;

/// The canonical tokens of a compiling `file`, equal across style and local binder names.
#[must_use]
pub fn canonicalize(file: syn::File) -> TokenStream {
    canonical(file, true)
}

/// The canonical tokens of a `file` meant to fail, retaining spellings whose equivalence requires compilation.
#[must_use]
pub fn canonicalize_failing(file: syn::File) -> TokenStream {
    canonical(file, false)
}

fn canonical(mut file: syn::File, compiles: bool) -> TokenStream {
    drift::normalize_file(&mut file, compiles);
    alpha::normalize_file(&mut file);
    fold_tokens(file.to_token_stream(), false, false)
        .into_iter()
        .collect()
}

/// The std macros that accept an optional trailing comma and treat it as nothing.
const COMMA_BLIND_MACROS: [&str; 21] = [
    "assert",
    "assert_eq",
    "assert_ne",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "eprint",
    "eprintln",
    "format",
    "format_args",
    "matches",
    "panic",
    "print",
    "println",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// The built-in attributes whose arguments a trailing comma doesn't change.
const COMMA_BLIND_ATTRIBUTES: [&str; 10] = [
    "allow", "cfg", "cfg_attr", "deny", "derive", "expect", "feature", "forbid", "repr", "warn",
];

/// Drop each group's trailing comma unless `keep_commas`, set inside the tokens of a macro or an
/// attribute that may match on it, and respell literals outside `opaque` macro and attribute tokens.
fn fold_tokens(stream: TokenStream, opaque: bool, keep_commas: bool) -> Vec<TokenTree> {
    let mut out: Vec<TokenTree> = Vec::new();
    // A proc macro derive was seen, then the `struct`, `enum` or `union` whose body it reads.
    let (mut derived, mut derived_item) = (false, false);
    for tree in stream {
        let tree = match tree {
            TokenTree::Ident(ident) => {
                derived_item |= derived && ["struct", "enum", "union"].iter().any(|kw| ident == kw);
                TokenTree::Ident(ident)
            }
            TokenTree::Punct(punct) => {
                if punct.as_char() == ';' {
                    (derived, derived_item) = (false, false);
                }
                TokenTree::Punct(punct)
            }
            TokenTree::Group(group) => {
                let shut = opaque
                    || matches!(out.last(), Some(TokenTree::Punct(p)) if matches!(p.as_char(), '!' | '#'));
                derived |= macro_derive_attribute(&group);
                let body = derived_item
                    && matches!(
                        group.delimiter(),
                        proc_macro2::Delimiter::Brace | proc_macro2::Delimiter::Parenthesis
                    );
                if body {
                    (derived, derived_item) = (false, false);
                }
                let keep = keep_commas || body || reads_commas(&out, &group);
                drift::map_group(&group, |inner| {
                    let mut inner = fold_tokens(inner, shut, keep);
                    // A one-tuple keeps its comma, the paren fold already told it apart.
                    if !keep
                        && matches!(inner.last(), Some(TokenTree::Punct(p)) if p.as_char() == ',')
                    {
                        inner.pop();
                    }
                    inner.into_iter().collect()
                })
            }
            TokenTree::Literal(lit) if !opaque => drift::canonical_literal(lit),
            TokenTree::Literal(lit) => TokenTree::Literal(lit),
        };
        out.push(tree);
    }
    out
}

/// Whether `group` is a `[derive(…)]` naming a proc macro. Only an attribute puts one right
/// before an item keyword, so the `#` ahead of it needs no check.
fn macro_derive_attribute(group: &proc_macro2::Group) -> bool {
    let mut inner = group.stream().into_iter();
    match (inner.next(), inner.next()) {
        (Some(TokenTree::Ident(name)), Some(TokenTree::Group(list))) if name == "derive" => {
            list.stream().into_iter().any(|tree| {
                matches!(tree, TokenTree::Ident(name)
                    if !drift::STD_DERIVES.iter().any(|std| name == std))
            })
        }
        _ => false,
    }
}

/// Whether `group`, following the tokens in `before`, holds the input of a macro call or an
/// attribute that may match on a trailing comma.
fn reads_commas(before: &[TokenTree], group: &proc_macro2::Group) -> bool {
    let back = |n: usize| before.len().checked_sub(n).and_then(|i| before.get(i));
    let punct = |tree: Option<&TokenTree>, c: char| matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == c);
    if punct(back(1), '!')
        && let Some(TokenTree::Ident(name)) = back(2)
    {
        return !COMMA_BLIND_MACROS.iter().any(|blind| name == blind);
    }
    let attribute = group.delimiter() == proc_macro2::Delimiter::Bracket
        && (punct(back(1), '#') || (punct(back(1), '!') && punct(back(2), '#')));
    if !attribute {
        return false;
    }
    let mut inner = group.stream().into_iter();
    let builtin = match (inner.next(), inner.next()) {
        (Some(TokenTree::Ident(name)), next) => {
            !punct(next.as_ref(), ':') && COMMA_BLIND_ATTRIBUTES.iter().any(|blind| name == blind)
        }
        _ => false,
    };
    !builtin
}

#[cfg(test)]
mod tests {
    use super::canonicalize as canon;
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// Canonical text of one test body.
    struct Canon {
        text: String,
    }

    /// `code` parsed the way a doctest compiles, panicking when it does not parse.
    fn parse(code: &str) -> syn::File {
        let is_main = |item: &syn::Item| matches!(item, syn::Item::Fn(f) if f.sig.ident == "main");
        match syn::parse_str::<syn::File>(code) {
            Ok(file) if file.items.iter().any(is_main) => file,
            _ => syn::parse_str::<syn::File>(&format!("fn main() {{ {code}\n}}"))
                .unwrap_or_else(|e| panic!("{code}: {e}")),
        }
    }

    /// `code` canonicalized the way a doctest compiles.
    fn canonicalize(code: &str) -> Canon {
        Canon {
            text: canon(parse(code)).to_string(),
        }
    }

    #[test]
    fn a_runtime_relative_path_is_its_own_content() {
        let a = r#"let f = File::open("../data.csv").unwrap();"#;
        let b = r#"let f = File::open("data.csv").unwrap();"#;
        assert_ne!(canonicalize(a).text, canonicalize(b).text);
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
    fn a_radix_literal_whose_suffix_misreads_in_decimal_keeps_its_spelling() {
        // In decimal `0b0buu` reads as a binary prefix, `0o7e3` as a float,
        // `0b0x1` and `0b0x0` as hex literals.
        for literal in ["0b0buu", "0o7e3", "0b0x1", "0b0x0"] {
            assert!(canonicalize(literal).text.contains(literal), "{literal}");
        }
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
        assert_eq!(canonicalize("1_000.50").text, "fn _canon_0 () { 1000.5 }");
        assert_eq!(canonicalize("1.0E+03").text, "fn _canon_0 () { 1.0e3 }");
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
    fn a_trailing_comma_a_macro_may_match_on_stays() {
        // rustfmt keeps these commas for the same reason, the macro sees them.
        for (with, without) in [
            ("my_macro!(a, b,);", "my_macro!(a, b);"),
            ("my_macro!((a, b,));", "my_macro!((a, b));"),
            (
                "tree! { 'a' => { 'b', 'c', } };",
                "tree! { 'a' => { 'b', 'c' } };",
            ),
            (
                "#[my_attr(a, b,)]\nfn f() {}",
                "#[my_attr(a, b)]\nfn f() {}",
            ),
        ] {
            assert_ne!(
                canonicalize(with).text,
                canonicalize(without).text,
                "{with}"
            );
        }
    }

    #[test]
    fn a_trailing_comma_a_std_macro_or_attribute_ignores_merges() {
        for (with, without) in [
            ("assert_eq!(a, b,);", "assert_eq!(a, b);"),
            ("std::println!(\"{}\", x,);", "std::println!(\"{}\", x);"),
            (
                "assert!(matches!(x, Some(1),));",
                "assert!(matches!(x, Some(1)));",
            ),
            (
                "#[cfg(any(unix, windows,))]\nfn f() {}",
                "#[cfg(any(unix, windows))]\nfn f() {}",
            ),
            ("let a = [1, 2,];", "let a = [1, 2];"),
            ("let a = ![true, false,];", "let a = ![true, false];"),
            (
                "#[repr(C)]\nstruct S {\n    x: u8,\n}",
                "#[repr(C)]\nstruct S {\n    x: u8\n}",
            ),
            (
                "#[derive(serde::Serialize)]\nstruct U;\nfn f(a: u8,) {}",
                "#[derive(serde::Serialize)]\nstruct U;\nfn f(a: u8) {}",
            ),
        ] {
            assert_eq!(
                canonicalize(with).text,
                canonicalize(without).text,
                "{with}"
            );
        }
    }

    #[test]
    fn the_input_of_a_proc_macro_derive_keeps_its_docs_lints_and_commas() {
        // clap reads field docs as help text, num_enum tests a derive that ignores extra attributes.
        for (one, two) in [
            (
                "#[derive(clap::Parser)]\nstruct A {\n    /// Help.\n    x: u8,\n}",
                "#[derive(clap::Parser)]\nstruct A {\n    x: u8,\n}",
            ),
            (
                "#[derive(TryFromPrimitive)]\nenum E {\n    Zero,\n    #[allow(unused)]\n    One,\n}",
                "#[derive(TryFromPrimitive)]\nenum E {\n    Zero,\n    One,\n}",
            ),
            (
                "#[derive(serde::Serialize)]\nstruct P {\n    x: u8,\n}",
                "#[derive(serde::Serialize)]\nstruct P {\n    x: u8\n}",
            ),
            (
                "/// About.\n#[derive(Clone, clap::Parser)]\nstruct A(u8);",
                "#[derive(Clone, clap::Parser)]\nstruct A(u8);",
            ),
            (
                "#[derive(serde::Serialize)]\npub(crate) struct P {\n    x: u8,\n}",
                "#[derive(serde::Serialize)]\npub(crate) struct P {\n    x: u8\n}",
            ),
            (
                "#[derive(serde::Serialize)]\nstruct P<T> {\n    x: T,\n}",
                "#[derive(serde::Serialize)]\nstruct P<T> {\n    x: T\n}",
            ),
        ] {
            assert_ne!(canonicalize(one).text, canonicalize(two).text, "{one}");
        }
    }

    #[test]
    fn the_input_of_std_derives_keeps_every_fold() {
        let a = canonicalize(
            "#[derive(Clone, Debug)]\nstruct P {\n    /// Doc.\n    #[allow(dead_code)]\n    x: u8,\n}",
        );
        let b = canonicalize("#[derive(Debug, Clone)]\nstruct P {\n    x: u8\n}");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_doc_attribute_holding_a_macro_call_stays() {
        // docify and document-features compute docs at compile time, and the call can fail.
        assert_ne!(
            canonicalize("#[doc = docify::embed!(\"a.rs\", x)]\npub struct S;").text,
            canonicalize("pub struct S;").text
        );
        assert_ne!(
            canonicalize("#[doc = include_str!(\"a.md\")]\npub struct S;").text,
            canonicalize("#[doc = include_str!(\"b.md\")]\npub struct S;").text
        );
        assert_eq!(
            canonicalize("#[doc = \"Text.\"]\npub struct S;").text,
            canonicalize("pub struct S;").text
        );
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
    fn a_block_arm_comma_merges() {
        let a = canonicalize("match 1 { 1 => {}, _ => {} }");
        let b = canonicalize("match 1 { 1 => {} _ => {} }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_unwrapped_arm_block_merges_with_the_comma_form() {
        let a = canonicalize("match v { 1 => { foo() } _ => 0 }");
        let b = canonicalize("match v { 1 => foo(), _ => 0 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_tail_return_arm_block_merges_with_the_comma_form() {
        let a = canonicalize("fn f(v: u8) -> u8 { match v { 1 => { return 2; } _ => 0 } }");
        let b = canonicalize("fn f(v: u8) -> u8 { match v { 1 => 2, _ => 0 } }");
        assert_eq!(a.text, b.text);
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
        assert_eq!(a.text, b.text);
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
    fn fn_with_different_name_stays_distinct_from_bare() {
        let with_run = canonicalize("fn run() {}");
        let bare = canonicalize("()");
        assert_ne!(with_run.text, bare.text);
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
    fn a_trailing_unit_return_folds_away() {
        let plain = canonicalize("fn f() { g(); }");
        assert_eq!(canonicalize("fn f() { g(); return; }").text, plain.text);
        assert_eq!(canonicalize("fn f() { g(); return }").text, plain.text);
        assert_eq!(
            canonicalize("fn f() { return; }").text,
            canonicalize("fn f() {}").text
        );
        assert_eq!(
            canonicalize("fn f(c: bool) { if c { g(); return; } else { h(); } }").text,
            canonicalize("fn f(c: bool) { if c { g(); } else { h(); } }").text
        );
    }

    #[test]
    fn an_attributed_or_early_unit_return_stays() {
        let plain = canonicalize("fn f() { g(); }");
        assert_ne!(
            canonicalize("fn f() { g(); #[cfg(unix)] return; }").text,
            plain.text
        );
        assert_ne!(canonicalize("fn f() { return; g(); }").text, plain.text);
    }

    #[test]
    fn a_return_arm_of_a_tail_match_folds() {
        let plain = canonicalize("fn f(v: u8) -> u8 { match v { 1 => 2, _ => 0 } }");
        for arm in ["1 => return 2,", "1 => { return 2 }"] {
            let source = format!("fn f(v: u8) -> u8 {{ match v {{ {arm} _ => 0 }} }}");
            assert_eq!(canonicalize(&source).text, plain.text, "{arm}");
        }
    }

    #[test]
    fn an_attributed_return_arm_stays() {
        let plain = canonicalize("fn f(v: u8) -> u8 { match v { 1 => 2, _ => 0 } }");
        let attributed =
            canonicalize("fn f(v: u8) -> u8 { match v { 1 => { #[cfg(unix)] return 2 } _ => 0 } }");
        assert_ne!(attributed.text, plain.text);
    }

    #[test]
    fn an_else_block_holding_only_an_if_collapses() {
        assert_eq!(
            canonicalize("let x = if a { 1 } else { if b { 2 } else { 3 } };").text,
            canonicalize("let x = if a { 1 } else if b { 2 } else { 3 };").text
        );
        assert_eq!(
            canonicalize("if a { f(); } else { if b { g(); } }").text,
            canonicalize("if a { f(); } else if b { g(); }").text
        );
    }

    #[test]
    fn an_attributed_if_in_an_else_block_stays() {
        assert_ne!(
            canonicalize("if a { f(); } else { #[cfg(unix)] if b { g(); } }").text,
            canonicalize("if a { f(); } else if b { g(); }").text
        );
    }

    #[test]
    fn rustfmt_attributes_are_inert() {
        let plain = canonicalize("fn f() {}");
        assert_eq!(canonicalize("#[rustfmt::skip]\nfn f() {}").text, plain.text);
        assert_eq!(
            canonicalize("#[rustfmt::skip::macros(vec)]\nfn f() {}").text,
            plain.text
        );
    }

    #[test]
    fn other_tool_or_lookalike_attributes_stay() {
        let plain = canonicalize("async fn f() {}");
        for attr in ["#[rustfmt_skip]", "#[tokio::main]"] {
            let source = format!("{attr}\nasync fn f() {{}}");
            assert_ne!(canonicalize(&source).text, plain.text, "{attr}");
        }
    }

    #[test]
    fn inline_bounds_merge_with_their_where_form() {
        let pairs = [
            (
                "fn f<T: Clone>(t: T) -> T { t.clone() }",
                "fn f<T>(t: T) -> T where T: Clone { t.clone() }",
            ),
            (
                "trait Tr {}\nstruct S<T>(T);\nimpl<T: Clone> Tr for S<T> {}",
                "trait Tr {}\nstruct S<T>(T);\nimpl<T> Tr for S<T> where T: Clone {}",
            ),
            ("struct S<T: Clone>(T);", "struct S<T>(T) where T: Clone;"),
            (
                "fn f<'a, 'b: 'a>(x: &'a u8, y: &'b u8) {}",
                "fn f<'a, 'b>(x: &'a u8, y: &'b u8) where 'b: 'a {}",
            ),
            (
                "fn f<T: Clone>(t: T) where T: Copy {}",
                "fn f<T>(t: T) where T: Clone, T: Copy {}",
            ),
        ];
        for (inline, clause) in pairs {
            assert_eq!(
                canonicalize(inline).text,
                canonicalize(clause).text,
                "{inline}"
            );
        }
    }

    #[test]
    fn an_impl_trait_argument_stays_apart_from_a_generic_parameter() {
        assert_ne!(
            canonicalize("fn f(x: impl Clone) {}").text,
            canonicalize("fn f<T: Clone>(x: T) {}").text
        );
    }

    #[test]
    fn elidable_lifetimes_merge_with_their_elided_form() {
        let pairs = [
            ("const S: &'static str = \"a\";", "const S: &str = \"a\";"),
            (
                "static S: &'static [&'static str] = &[];",
                "static S: &[&str] = &[];",
            ),
            (
                "const F: &'static dyn Fn(&'static str) = &g;",
                "const F: &dyn Fn(&'static str) = &g;",
            ),
            (
                "fn f(x: &'_ str) -> usize { x.len() }",
                "fn f(x: &str) -> usize { x.len() }",
            ),
            ("fn f(x: Wrap<'_>) {}", "fn f(x: Wrap) {}"),
            ("fn f(x: Wrap<'_, u8>) {}", "fn f(x: Wrap<u8>) {}"),
        ];
        for (written, elided) in pairs {
            assert_eq!(
                canonicalize(written).text,
                canonicalize(elided).text,
                "{written}"
            );
        }
    }

    #[test]
    fn lifetimes_whose_elision_means_something_else_stay() {
        let pairs = [
            // In a fn pointer or `Fn` sugar elision is higher ranked, not `'static`.
            ("const F: fn(&'static str) = g;", "const F: fn(&str) = g;"),
            (
                "const F: &dyn Fn(&'static str) = &g;",
                "const F: &dyn Fn(&str) = &g;",
            ),
            // An impl header may not elide a path lifetime (E0726).
            (
                "trait Tr {}\nimpl Tr for Wrap<'_> {}",
                "trait Tr {}\nimpl Tr for Wrap {}",
            ),
            ("fn f<'a>(x: &'a str) {}", "fn f(x: &str) {}"),
        ];
        for (written, other) in pairs {
            assert_ne!(
                canonicalize(written).text,
                canonicalize(other).text,
                "{written}"
            );
        }
    }

    #[test]
    fn std_derives_merge_in_any_order_or_split() {
        let pairs = [
            (
                "#[derive(Debug, Clone)]\nstruct S;",
                "#[derive(Clone, Debug)]\nstruct S;",
            ),
            (
                "#[derive(Debug)]\n#[derive(Clone)]\nstruct S;",
                "#[derive(Clone, Debug)]\nstruct S;",
            ),
            (
                "#[derive(Debug)]\n#[repr(C)]\n#[derive(Clone)]\nstruct S;",
                "#[derive(Clone, Debug)]\n#[repr(C)]\nstruct S;",
            ),
            (
                "#[derive(PartialEq, Eq, Debug)]\nenum E { A }",
                "#[derive(Debug, Eq, PartialEq)]\nenum E { A }",
            ),
        ];
        for (written, sorted) in pairs {
            assert_eq!(
                canonicalize(written).text,
                canonicalize(sorted).text,
                "{written}"
            );
        }
    }

    #[test]
    fn a_derive_list_with_a_proc_macro_keeps_its_order() {
        assert_ne!(
            canonicalize("#[derive(Serialize, Debug)]\nstruct S;").text,
            canonicalize("#[derive(Debug, Serialize)]\nstruct S;").text
        );
    }

    #[test]
    fn a_leading_colon_on_an_unbound_crate_path_folds() {
        let pairs = [
            (
                "let v = ::std::vec::Vec::<u8>::new();",
                "let v = std::vec::Vec::<u8>::new();",
            ),
            (
                "let v: ::std::vec::Vec<u8> = Vec::new();",
                "let v: std::vec::Vec<u8> = Vec::new();",
            ),
            (
                "use ::std::fmt;\nlet _ = fmt::Error;",
                "use std::fmt;\nlet _ = fmt::Error;",
            ),
            ("::std::println!(\"x\");", "std::println!(\"x\");"),
        ];
        for (rooted, plain) in pairs {
            assert_eq!(
                canonicalize(rooted).text,
                canonicalize(plain).text,
                "{rooted}"
            );
        }
    }

    #[test]
    fn a_leading_colon_past_a_local_item_of_that_name_stays() {
        assert_ne!(
            canonicalize("mod std { pub fn f() {} }\n::std::mem::drop(1);").text,
            canonicalize("mod std { pub fn f() {} }\nstd::mem::drop(1);").text
        );
    }

    #[test]
    fn grammar_drift_empty_path_arguments() {
        for (a, b) in [
            ("struct S; type A = S<>;", "struct S; type A = S;"),
            (
                "fn f() {} fn main() { f::<>(); }",
                "fn f() {} fn main() { f(); }",
            ),
            (
                "struct S; impl S { fn f(&self) {} } fn main() { S.f::<>(); }",
                "struct S; impl S { fn f(&self) {} } fn main() { S.f(); }",
            ),
            (
                "trait T {} struct S; impl T<> for S<> {}",
                "trait T {} struct S; impl T for S {}",
            ),
            (
                "struct S<'a>(&'a u8); impl S<> {}",
                "struct S<'a>(&'a u8); impl S {}",
            ),
            (
                "fn f<T>(x: T) { missing(x); } fn main() { f::<>(7); }",
                "fn f<T>(x: T) { missing(x); } fn main() { f(7); }",
            ),
        ] {
            assert_eq!(canonicalize(a).text, canonicalize(b).text, "{a}");
            assert_eq!(
                super::canonicalize_failing(parse(a)).to_string(),
                super::canonicalize_failing(parse(b)).to_string(),
                "{a}"
            );
        }
    }

    #[test]
    fn empty_arguments_preserve_values_captures_and_macro_inputs() {
        for (a, b) in [
            ("fn f(x: Vec<u8>) {}", "fn f(x: Vec<u16>) {}"),
            (
                "fn f<T>(x: T) {} fn main() { f::<u8>(7); }",
                "fn f<T>(x: T) {} fn main() { f::<u16>(7); }",
            ),
            (
                "fn f(_: &u8) -> impl Sized + use<> { 7u8 }",
                "fn f(_: &u8) -> impl Sized { 7u8 }",
            ),
            ("#[inspect] type A = S<>;", "#[inspect] type A = S;"),
            (
                "fn f() { #[inspect] let x: S<>; }",
                "fn f() { #[inspect] let x: S; }",
            ),
            (
                "fn f() { let x = #[inspect] g::<>(); }",
                "fn f() { let x = #[inspect] g(); }",
            ),
            (
                "#[inspect] fn f() { x.read::<>(); }",
                "#[inspect] fn f() { x.read(); }",
            ),
            (
                "#[derive(Inspect)] struct S(T<>);",
                "#[derive(Inspect)] struct S(T);",
            ),
            ("inspect!(f::<>());", "inspect!(f());"),
            ("fn f() { x.read::<u8>(); }", "fn f() { x.read::<u16>(); }"),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
    }

    #[test]
    fn grammar_drift_default_abi() {
        assert_merge(&[
            (
                "extern fn f(x: u8) -> u8 { x }",
                "extern \"C\" fn f(x: u8) -> u8 { x }",
            ),
            (
                "type F = unsafe extern fn(u8) -> u8;",
                "type F = unsafe extern \"C\" fn(u8) -> u8;",
            ),
            (
                "unsafe extern { fn f(x: u8) -> u8; }",
                "unsafe extern \"C\" { fn f(x: u8) -> u8; }",
            ),
        ]);
    }

    #[test]
    fn grammar_drift_function_trait_unit_output() {
        assert_merge(&[
            (
                "fn f<T: Fn(u8) -> ()>(x: T) { x(7); }",
                "fn f<T: Fn(u8)>(x: T) { x(7); }",
            ),
            (
                "fn f<T: FnMut() -> ()>(mut x: T) { x(); }",
                "fn f<T: FnMut()>(mut x: T) { x(); }",
            ),
            (
                "fn f<T: FnOnce() -> ()>(x: T) { x(); }",
                "fn f<T: FnOnce()>(x: T) { x(); }",
            ),
            ("type F = dyn Fn(u8) -> ();", "type F = dyn Fn(u8);"),
        ]);
    }

    #[test]
    fn grammar_drift_restricted_visibility() {
        assert_merge(&[
            ("pub(in crate) struct S;", "pub(crate) struct S;"),
            (
                "mod m { pub(in self) fn f() {} }",
                "mod m { pub(self) fn f() {} }",
            ),
            (
                "mod m { pub(in super) struct S; }",
                "mod m { pub(super) struct S; }",
            ),
        ]);
    }

    #[test]
    fn grammar_drift_value_receiver() {
        assert_merge(&[
            (
                "struct S(u8); impl S { fn f(self: Self) -> u8 { self.0 } }",
                "struct S(u8); impl S { fn f(self) -> u8 { self.0 } }",
            ),
            (
                "struct S(u8); impl S { fn f(mut self: Self) -> u8 { self.0 += 1; self.0 } }",
                "struct S(u8); impl S { fn f(mut self) -> u8 { self.0 += 1; self.0 } }",
            ),
        ]);
    }

    #[test]
    fn grammar_drift_leading_pattern_pipe() {
        assert_merge(&[
            (
                "fn f(x: u8) -> u8 { match x { | 0 | 1 => 7, _ => 9 } }",
                "fn f(x: u8) -> u8 { match x { 0 | 1 => 7, _ => 9 } }",
            ),
            (
                "fn f(x: (u8,)) -> u8 { match x { (| 0 | 1,) => 7, _ => 9 } }",
                "fn f(x: (u8,)) -> u8 { match x { (0 | 1,) => 7, _ => 9 } }",
            ),
        ]);
    }

    #[test]
    fn grammar_drift_wildcard_binding() {
        assert_merge(&[
            ("fn f(x @ _: u8) -> u8 { x }", "fn f(x: u8) -> u8 { x }"),
            (
                "fn f(x: &u8) -> u8 { let y @ _ = x; *y }",
                "fn f(x: &u8) -> u8 { let y = x; *y }",
            ),
            (
                "fn f(x: u8) -> u8 { match x { ref y @ _ => *y } }",
                "fn f(x: u8) -> u8 { match x { ref y => *y } }",
            ),
        ]);
    }

    #[test]
    fn grammar_drift_generic_list_commas() {
        for (a, b) in [
            ("struct S<T,>(T);", "struct S<T>(T);"),
            (
                "fn f<'a, T, const N: usize,>(x: &'a T) {}",
                "fn f<'a, T, const N: usize>(x: &'a T) {}",
            ),
            ("fn f(x: Vec<u8,>) {}", "fn f(x: Vec<u8>) {}"),
            ("fn f() { g::<u8,>(); }", "fn f() { g::<u8>(); }"),
            ("fn f() { x.g::<u8,>(); }", "fn f() { x.g::<u8>(); }"),
        ] {
            assert_eq!(canonicalize(a).text, canonicalize(b).text, "{a}");
            assert_eq!(
                super::canonicalize_failing(parse(a)).to_string(),
                super::canonicalize_failing(parse(b)).to_string(),
                "{a}"
            );
        }
    }

    #[test]
    fn grammar_drift_closure_parameter_commas() {
        let a = "fn f() { let c = |x: u8,| x + 1; c(7); }";
        let b = "fn f() { let c = |x: u8| x + 1; c(7); }";
        assert_eq!(canonicalize(a).text, canonicalize(b).text);
        assert_eq!(
            super::canonicalize_failing(parse(a)).to_string(),
            super::canonicalize_failing(parse(b)).to_string()
        );
    }

    #[test]
    fn grammar_drift_unit_return_value() {
        assert_merge(&[
            ("fn f() { return (); }", "fn f() { return; }"),
            (
                "fn f(x: bool) { if x { return (); } let _ = x; }",
                "fn f(x: bool) { if x { return; } let _ = x; }",
            ),
        ]);
    }

    #[test]
    fn grammar_commas_and_returns_preserve_inputs_and_failures() {
        for (a, b) in [
            ("#[inspect] struct S<T,>(T);", "#[inspect] struct S<T>(T);"),
            ("struct S<#[inspect] T,>(T);", "struct S<#[inspect] T>(T);"),
            (
                "#[inspect] fn f() { g::<u8,>(); }",
                "#[inspect] fn f() { g::<u8>(); }",
            ),
            (
                "#[inspect] fn f() { x.g::<u8,>(); }",
                "#[inspect] fn f() { x.g::<u8>(); }",
            ),
            (
                "#[inspect] fn f() { let c = |x,| x; }",
                "#[inspect] fn f() { let c = |x| x; }",
            ),
            (
                "fn f() { let c = |#[inspect] x,| x; }",
                "fn f() { let c = |#[inspect] x| x; }",
            ),
            (
                "#[inspect] fn f() { return (); }",
                "#[inspect] fn f() { return; }",
            ),
            ("fn f() { g::<u8, u16>(); }", "fn f() { g::<u16, u8>(); }"),
            ("fn f() { let _ = (7,); }", "fn f() { let _ = (7); }"),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
        let a = "fn f() -> ! { return (); }";
        let b = "fn f() -> ! { return; }";
        assert_ne!(
            super::canonicalize_failing(parse(a)).to_string(),
            super::canonicalize_failing(parse(b)).to_string()
        );
    }

    #[test]
    fn grammar_folds_preserve_failure_distinctions() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        for (a, b) in [
            (
                "#![deny(missing_abi)] extern fn f() {}",
                "#![deny(missing_abi)] extern \"C\" fn f() {}",
            ),
            (
                "const c: u8 = 7; fn f() { let c @ _ = 7; }",
                "const c: u8 = 7; fn f() { let c = 7; }",
            ),
        ] {
            assert_ne!(failing(a), failing(b), "{a}");
        }
        for (a, b) in [
            (
                "fn f<T: Fn() -> ()>(x: T) { missing(x); }",
                "fn f<T: Fn()>(x: T) { missing(x); }",
            ),
            (
                "pub(in crate) fn f() { missing(); }",
                "pub(crate) fn f() { missing(); }",
            ),
            (
                "struct S; impl S { fn f(mut self: Self) { missing(self); } }",
                "struct S; impl S { fn f(mut self) { missing(self); } }",
            ),
        ] {
            assert_eq!(failing(a), failing(b), "{a}");
        }
    }

    #[test]
    fn grammar_folds_preserve_semantic_neighbors() {
        for (a, b) in [
            ("extern \"C\" fn f() {}", "fn f() {}"),
            (
                "type F = extern \"C\" fn();",
                "type F = extern \"C-unwind\" fn();",
            ),
            (
                "type F = extern \"C\" fn();",
                "type F = extern \"system\" fn();",
            ),
            (
                "type F = extern \"C\" fn();",
                "type F = unsafe extern \"C\" fn();",
            ),
            (
                "fn f<T: Fn() -> u8>(x: T) { x(); }",
                "fn f<T: Fn()>(x: T) { x(); }",
            ),
            (
                "mod m { pub(in crate) struct S; }",
                "mod m { pub(in self) struct S; }",
            ),
            (
                "struct S; impl S { fn f(self: Box<Self>) {} }",
                "struct S; impl S { fn f(self) {} }",
            ),
            (
                "struct S; impl S { fn f(self: &mut Self) {} }",
                "struct S; impl S { fn f(self) {} }",
            ),
            (
                "fn f(x: u8) { match x { y @ 7 => (), _ => () } }",
                "fn f(x: u8) { match x { y => (), _ => () } }",
            ),
            (
                "fn f(x: u8) { match x { ref y @ _ => (), } }",
                "fn f(x: u8) { match x { y => (), } }",
            ),
            (
                "fn f(x: (u8, u8)) { let (Upper @ _, lower) = x; }",
                "fn f(x: (u8, u8)) { let (Upper, lower) = x; }",
            ),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
    }

    #[test]
    fn grammar_folds_preserve_macro_item_inputs() {
        for (a, b) in [
            (
                "#[inspect] extern fn f() {}",
                "#[inspect] extern \"C\" fn f() {}",
            ),
            (
                "#[inspect] fn f<T: Fn() -> ()>(x: T) {}",
                "#[inspect] fn f<T: Fn()>(x: T) {}",
            ),
            (
                "#[inspect] pub(in crate) struct S;",
                "#[inspect] pub(crate) struct S;",
            ),
            (
                "struct S; impl S { #[inspect] fn f(self: Self) {} }",
                "struct S; impl S { #[inspect] fn f(self) {} }",
            ),
            (
                "#[inspect] fn f(x: u8) { match x { | 0 | 1 => (), _ => () } }",
                "#[inspect] fn f(x: u8) { match x { 0 | 1 => (), _ => () } }",
            ),
            ("#[inspect] fn f(x @ _: u8) {}", "#[inspect] fn f(x: u8) {}"),
            (
                "#[derive(Inspect)] struct S(extern fn());",
                "#[derive(Inspect)] struct S(extern \"C\" fn());",
            ),
            ("inspect!(extern fn());", "inspect!(extern \"C\" fn());"),
            ("inspect!(x @ _);", "inspect!(x);"),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
    }

    #[test]
    fn grammar_folds_preserve_nested_macro_inputs() {
        for (a, b) in [
            (
                "fn f() { #[inspect] let x: extern fn(); }",
                "fn f() { #[inspect] let x: extern \"C\" fn(); }",
            ),
            (
                "fn f() { #[inspect] { let x: extern fn(); } }",
                "fn f() { #[inspect] { let x: extern \"C\" fn(); } }",
            ),
            (
                "fn f(x: u8) { match x { #[inspect] | 0 | 1 => (), _ => () } }",
                "fn f(x: u8) { match x { #[inspect] 0 | 1 => (), _ => () } }",
            ),
            ("fn f(#[inspect] x @ _: u8) {}", "fn f(#[inspect] x: u8) {}"),
            (
                "struct S { #[inspect] x: extern fn() }",
                "struct S { #[inspect] x: extern \"C\" fn() }",
            ),
            (
                "enum S { #[inspect] V(extern fn()) }",
                "enum S { #[inspect] V(extern \"C\" fn()) }",
            ),
            (
                "fn f<#[inspect] T: Fn() -> ()>(x: T) {}",
                "fn f<#[inspect] T: Fn()>(x: T) {}",
            ),
            (
                "#[inspect = 0 as extern fn()] const S: u8 = 7;",
                "#[inspect = 0 as extern \"C\" fn()] const S: u8 = 7;",
            ),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
    }

    #[test]
    fn grammar_macro_scope_ends_at_its_node() {
        assert_merge(&[
            (
                "#[inspect] fn held() {} extern fn ordinary() {}",
                "#[inspect] fn held() {} extern \"C\" fn ordinary() {}",
            ),
            (
                "fn f() { #[inspect] let held: extern fn(); let ordinary: extern fn(); }",
                "fn f() { #[inspect] let held: extern fn(); let ordinary: extern \"C\" fn(); }",
            ),
            (
                "fn f() { let held = #[inspect] { 7 }; let ordinary: extern fn(); }",
                "fn f() { let held = #[inspect] { 7 }; let ordinary: extern \"C\" fn(); }",
            ),
        ]);
    }

    #[test]
    fn grammar_folds_preserve_file_and_argument_inputs() {
        for (a, b) in [
            (
                "#![inspect] fn main() { let x: extern fn(); }",
                "#![inspect] fn main() { let x: extern \"C\" fn(); }",
            ),
            (
                "type F = fn(#[inspect] x: extern fn());",
                "type F = fn(#[inspect] x: extern \"C\" fn());",
            ),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a}");
        }
    }

    #[test]
    fn spelling_drift_type_position_turbofish() {
        assert_merge(&[
            ("type A = Vec::<u8>;", "type A = Vec<u8>;"),
            (
                "type A = <Vec::<u8> as IntoIterator>::Item;",
                "type A = <Vec<u8> as IntoIterator>::Item;",
            ),
            (
                "fn f(x: Option::<Vec::<u8>>) -> usize { x.map_or(0, |v| v.len()) }",
                "fn f(x: Option<Vec<u8>>) -> usize { x.map_or(0, |v| v.len()) }",
            ),
            (
                "trait Tr<T> {} fn f<T: Tr::<u8>>(x: T) {}",
                "trait Tr<T> {} fn f<T: Tr<u8>>(x: T) {}",
            ),
            (
                "trait Tr<T> {} struct S; impl Tr::<u8> for S {}",
                "trait Tr<T> {} struct S; impl Tr<u8> for S {}",
            ),
        ]);
    }

    #[test]
    fn spelling_drift_trailing_bound_separator() {
        assert_merge(&[
            (
                "fn f<T: Clone +>(x: T) -> T { x.clone() }",
                "fn f<T: Clone>(x: T) -> T { x.clone() }",
            ),
            (
                "fn f<T>(x: T) -> T where T: Clone + { x.clone() }",
                "fn f<T>(x: T) -> T where T: Clone { x.clone() }",
            ),
            ("trait T: Send + {}", "trait T: Send {}"),
            ("type A = dyn Send +;", "type A = dyn Send;"),
            (
                "fn f() -> impl Clone + { 1u8 }",
                "fn f() -> impl Clone { 1u8 }",
            ),
            ("trait T { type A: Clone +; }", "trait T { type A: Clone; }"),
            ("fn f<'a: 'static +>() {}", "fn f<'a: 'static>() {}"),
            ("trait A = Send +;", "trait A = Send;"),
            (
                "fn f<T: Iterator<Item: Clone +>>(x: T) {}",
                "fn f<T: Iterator<Item: Clone>>(x: T) {}",
            ),
        ]);
    }

    #[test]
    fn spelling_drift_parenthesized_single_bound() {
        assert_merge(&[
            (
                "fn f<T: (Clone)>(x: T) -> T { x.clone() }",
                "fn f<T: Clone>(x: T) -> T { x.clone() }",
            ),
            ("trait T: (Send) {}", "trait T: Send {}"),
            ("type A = dyn (Send);", "type A = dyn Send;"),
            (
                "fn f<T: Iterator<Item: (Clone)>>(x: T) {}",
                "fn f<T: Iterator<Item: Clone>>(x: T) {}",
            ),
        ]);
    }

    #[test]
    fn spelling_drift_function_pointer_unit_return() {
        assert_merge(&[
            ("type F = fn(u8) -> ();", "type F = fn(u8);"),
            (
                "type F = for<'a> unsafe extern \"C\" fn(&'a u8) -> ();",
                "type F = for<'a> unsafe extern \"C\" fn(&'a u8);",
            ),
            (
                "unsafe extern \"C\" { static F: fn(u8) -> (); fn take(f: fn() -> ()); }",
                "unsafe extern \"C\" { static F: fn(u8); fn take(f: fn()); }",
            ),
        ]);
    }

    #[test]
    fn spelling_drift_explicit_shared_receiver() {
        assert_merge(&[
            (
                "struct S(u8); impl S { fn f(self: &Self) -> u8 { self.0 } }",
                "struct S(u8); impl S { fn f(&self) -> u8 { self.0 } }",
            ),
            (
                "struct S(u8); impl S { fn f<'a>(self: &'a Self) -> &'a u8 { &self.0 } }",
                "struct S(u8); impl S { fn f<'a>(&'a self) -> &'a u8 { &self.0 } }",
            ),
        ]);
    }

    #[test]
    fn spelling_drift_preserves_opaque_macro_inputs() {
        for (a, b) in [
            ("inspect!(Vec::<u8>);", "inspect!(Vec<u8>);"),
            ("inspect!(T: Clone +);", "inspect!(T: Clone);"),
            ("inspect!(T: (Clone));", "inspect!(T: Clone);"),
            ("inspect!(fn(u8) -> ());", "inspect!(fn(u8));"),
            ("inspect!(self: &Self);", "inspect!(&self);"),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
    }

    #[test]
    fn spelling_drift_preserves_derive_inputs() {
        for (a, b) in [
            (
                "#[derive(Inspect)] struct S(Vec::<u8>);",
                "#[derive(Inspect)] struct S(Vec<u8>);",
            ),
            (
                "#[derive(Inspect)] struct S<T: Clone +>(T);",
                "#[derive(Inspect)] struct S<T: Clone>(T);",
            ),
            (
                "#[derive(Inspect)] struct S<T: (Clone)>(T);",
                "#[derive(Inspect)] struct S<T: Clone>(T);",
            ),
            (
                "#[derive(Inspect)] struct S(fn(u8) -> ());",
                "#[derive(Inspect)] struct S(fn(u8));",
            ),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
    }

    #[test]
    fn spelling_drift_preserves_attribute_inputs() {
        for (a, b) in [
            (
                "#[inspect] type A = Vec::<u8>;",
                "#[inspect] type A = Vec<u8>;",
            ),
            (
                "#[inspect] fn f<T: Clone +>(x: T) -> T { x.clone() }",
                "#[inspect] fn f<T: Clone>(x: T) -> T { x.clone() }",
            ),
            (
                "#[inspect] fn f<T: (Clone)>(x: T) -> T { x.clone() }",
                "#[inspect] fn f<T: Clone>(x: T) -> T { x.clone() }",
            ),
            (
                "#[inspect] type F = fn(u8) -> ();",
                "#[inspect] type F = fn(u8);",
            ),
            (
                "struct S; #[inspect] impl S { fn f(self: &Self) {} }",
                "struct S; #[inspect] impl S { fn f(&self) {} }",
            ),
            (
                "#[derive(Inspect)] struct S([u8; { type F = fn() -> (); 0 }]);",
                "#[derive(Inspect)] struct S([u8; { type F = fn(); 0 }]);",
            ),
            (
                "struct S; impl S { #[inspect] fn f(self: &Self) {} }",
                "struct S; impl S { #[inspect] fn f(&self) {} }",
            ),
            (
                "trait Tr { #[inspect] fn f(self: &Self); }",
                "trait Tr { #[inspect] fn f(&self); }",
            ),
            (
                "unsafe extern \"C\" { #[inspect] static F: fn(u8) -> (); }",
                "unsafe extern \"C\" { #[inspect] static F: fn(u8); }",
            ),
            (
                "unsafe extern \"C\" { #[inspect] fn take(f: fn() -> ()); }",
                "unsafe extern \"C\" { #[inspect] fn take(f: fn()); }",
            ),
            (
                "unsafe extern \"C\" { #[inspect = 1 as fn() -> ()] type T; }",
                "unsafe extern \"C\" { #[inspect = 1 as fn()] type T; }",
            ),
            (
                "unsafe extern \"C\" { #[inspect = 1 as fn() -> ()] m!(); }",
                "unsafe extern \"C\" { #[inspect = 1 as fn()] m!(); }",
            ),
            (
                "#[inspect] trait T: Send + {}",
                "#[inspect] trait T: Send {}",
            ),
            ("#[inspect] trait T = Send +;", "#[inspect] trait T = Send;"),
            (
                "trait T { #[inspect] type A: Clone +; }",
                "trait T { #[inspect] type A: Clone; }",
            ),
            (
                "#[inspect] fn f() -> impl Clone + { 1u8 }",
                "#[inspect] fn f() -> impl Clone { 1u8 }",
            ),
            (
                "#[inspect] type A = dyn Send +;",
                "#[inspect] type A = dyn Send;",
            ),
            (
                "#[inspect] fn f<T: Iterator<Item: Clone +>>(x: T) {}",
                "#[inspect] fn f<T: Iterator<Item: Clone>>(x: T) {}",
            ),
            (
                "trait Tr<T> {} struct S; #[inspect] impl Tr::<u8> for S {}",
                "trait Tr<T> {} struct S; #[inspect] impl Tr<u8> for S {}",
            ),
        ] {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
    }

    #[test]
    fn spelling_drift_preserves_verbatim_items() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        for (a, b) in [
            (
                "struct S; impl S { type A: Clone +; }",
                "struct S; impl S { type A: Clone; }",
            ),
            ("trait T { pub fn f() -> (); }", "trait T { pub fn f(); }"),
            (
                "unsafe extern \"C\" { type A: Clone +; }",
                "unsafe extern \"C\" { type A: Clone; }",
            ),
        ] {
            assert_ne!(failing(a), failing(b), "{a} vs {b}");
        }
    }

    #[test]
    fn spelling_drift_failing_programs_keep_equivalent_spellings() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        for (a, b) in [
            (
                "fn f(x: Vec::<u8>) { missing(x); }",
                "fn f(x: Vec<u8>) { missing(x); }",
            ),
            (
                "fn f<T: Clone +>(x: T) { missing(x); }",
                "fn f<T: Clone>(x: T) { missing(x); }",
            ),
            (
                "fn f<T: (Clone)>(x: T) { missing(x); }",
                "fn f<T: Clone>(x: T) { missing(x); }",
            ),
            (
                "type F = fn(u8) -> (); missing();",
                "type F = fn(u8); missing();",
            ),
            (
                "struct S; impl S { fn f(self: &Self) { missing(); } }",
                "struct S; impl S { fn f(&self) { missing(); } }",
            ),
        ] {
            assert_eq!(failing(a), failing(b), "{a} vs {b}");
        }
    }

    #[test]
    fn function_bounds_keep_output_grouping_parentheses() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        assert_ne!(
            failing("fn f<T: (Fn() -> &'static dyn core::fmt::Display) + Clone>(x: T) {}"),
            failing("fn f<T: Fn() -> &'static dyn core::fmt::Display + Clone>(x: T) {}")
        );
    }

    #[test]
    fn shared_and_mutable_receivers_keep_their_borrow_kind() {
        assert_ne!(
            canonicalize("struct S(u8); impl S { fn f(&self) -> u8 { self.0 } }").text,
            canonicalize("struct S(u8); impl S { fn f(&mut self) -> u8 { self.0 } }").text
        );
        assert_ne!(
            canonicalize("struct S(u8); impl S { fn f(self: &Self) -> u8 { self.0 } }").text,
            canonicalize("struct S(u8); impl S { fn f(self: &mut Self) -> u8 { self.0 } }").text
        );
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        assert_ne!(
            failing(
                "struct S; impl S { fn f<'a>(mut self: &'a Self, other: &'a Self) { self = other; } }"
            ),
            failing(
                "struct S; impl S { fn f<'a>(self: &'a Self, other: &'a Self) { self = other; } }"
            )
        );
        assert_ne!(
            failing("struct S; struct Other; impl S { fn f(self: &Other) {} }"),
            failing("struct S; struct Other; impl S { fn f(&self) {} }")
        );
        assert_ne!(
            failing("struct S; impl S { fn f(self: &[Self]) {} }"),
            failing("struct S; impl S { fn f(&self) {} }")
        );
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
    fn alpha_renames_let_bindings() {
        let a = canonicalize("let pino = 1;\npino + 1\n");
        let b = canonicalize("let abete = 1;\nabete + 1\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn raw_bindings_resolve_through_either_spelling() {
        assert_merge(&[
            ("let r#value = 2; f(value);", "let value = 2; f(value);"),
            ("let value = 2; f(r#value);", "let value = 2; f(value);"),
            (
                "fn f(r#value: u8) -> u8 { value }",
                "fn g(value: u8) -> u8 { value }",
            ),
            (
                "mod r#inner { pub fn r#value() -> u8 { 2 } } let n = inner::value();",
                "mod outer { pub fn result() -> u8 { 2 } } let n = outer::result();",
            ),
            (
                "struct r#Value; let _: Value = r#Value;",
                "struct Other; let _: Other = Other;",
            ),
            (
                "use std::mem::drop as r#dispose; dispose(2);",
                "use std::mem::drop as discard; discard(2);",
            ),
        ]);
    }

    #[test]
    fn raw_shadowing_uses_the_nearest_binding() {
        for raw in [
            "fn f() -> u8 { let x = 1; let r#x = 2; x }",
            "fn f() -> u8 { let r#x = 1; let x = 2; r#x }",
        ] {
            assert_eq!(
                canonicalize(raw).text,
                canonicalize("fn g() -> u8 { let a = 1; let a = 2; a }").text
            );
            assert_ne!(
                canonicalize(raw).text,
                canonicalize("fn g() -> u8 { let a = 1; let b = 2; a }").text
            );
        }
    }

    #[test]
    fn raw_identifier_macro_inputs_keep_their_spelling() {
        assert_split(&[
            (
                "let value = 2; opaque!(r#value);",
                "let value = 2; opaque!(value);",
            ),
            (
                "let r#value = 2; opaque!(value);",
                "let other = 2; opaque!(other);",
            ),
            (
                "let value = 1; let r#value = 2; opaque!(value);",
                "let value = 1; let other = 2; opaque!(value);",
            ),
            (
                "let value = 1; { let r#value = 2; opaque!(value); }",
                "let value = 1; { let other = 2; opaque!(value); }",
            ),
            (
                "struct r#Value; opaque!(::r#Value);",
                "struct r#Value; opaque!(r#Value);",
            ),
            (
                "mod inner { pub fn value() {} } opaque!(inner::r#value);",
                "mod inner { pub fn value() {} } opaque!(inner::value);",
            ),
            (
                "mod inner { pub fn r#value() {} } opaque!(inner::value);",
                "mod inner { pub fn other() {} } opaque!(inner::other);",
            ),
        ]);
    }

    #[test]
    fn raw_unit_patterns_keep_their_item_relationship() {
        assert_merge(&[(
            "struct r#Unit; fn f(x: Unit) { let r#Unit = x; }",
            "struct Other; fn g(x: Other) { let Other = x; }",
        )]);
    }

    #[test]
    fn raw_binding_resolution_preserves_compilation_failures() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        assert_eq!(
            failing("fn f() { let r#value = 2; missing(value); }"),
            failing("fn g() { let other = 2; missing(other); }")
        );
        assert_ne!(
            failing("fn f() { let value = 2; missing(r#unbound); }"),
            failing("fn g() { let other = 2; missing(other); }")
        );
        assert_ne!(
            failing("fn f<'r#static>(x: &'static u8) -> &'static u8 { x }"),
            failing("fn g<'a>(x: &'a u8) -> &'a u8 { x }")
        );
        assert_ne!(
            failing("'r#static: loop { break 'static; }"),
            failing("'good: loop { break 'good; }")
        );
    }

    #[test]
    fn alpha_renames_struct_shorthand_bindings() {
        let a = canonicalize("let P { pino } = make();\npino\n");
        let b = canonicalize("let P { pino: abete } = make();\nabete\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn alpha_renames_bindings_inside_an_at_subpattern() {
        let a = canonicalize("match Some(1) { pino @ Some(abete) => abete, None => 0 }");
        let b = canonicalize("match Some(1) { x @ Some(y) => y, None => 0 }");
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
        assert!(a.text.contains("_canon_0"));
        assert!(a.text.contains("_canon_1"));
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
        assert_eq!(a.text.matches("_canon_1").count(), 2);
        assert_eq!(a.text.matches("_canon_2").count(), 2);
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
    fn a_multibyte_char_before_a_width_name_keeps_the_rename() {
        // `€` is three bytes, the width name `w` starts after all of them.
        let a = canonicalize("let w = 5;\nprintln!(\"{:€w$}\", 1)\n");
        let b = canonicalize("let z = 5;\nprintln!(\"{:€z$}\", 1)\n");
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
    fn a_positional_identifier_argument_merges_with_its_inline_form() {
        let a = canonicalize("let x = 1;\nprintln!(\"{}\", x);\n");
        let b = canonicalize("let x = 1;\nprintln!(\"{x}\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_inlined_argument_keeps_its_spec() {
        let a = canonicalize("let x = 1;\nlet s = format!(\"{:?}\", x);\n");
        let b = canonicalize("let x = 1;\nlet s = format!(\"{x:?}\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn only_identifier_arguments_inline() {
        let a = canonicalize("let x = 1;\nwrite!(out, \"{} {}\", x, x + 1).unwrap();\n");
        let b = canonicalize("let x = 1;\nwrite!(out, \"{x} {}\", x + 1).unwrap();\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn inlining_composes_with_alpha_renaming() {
        let a = canonicalize("let pino = 1;\nprintln!(\"{}\", pino);\n");
        let b = canonicalize("let abete = 1;\nprintln!(\"{abete}\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_format_call_inside_a_macro_argument_inlines() {
        let a = canonicalize("let x = 1;\nassert_eq!(format!(\"{}\", x), \"1\");\n");
        let b = canonicalize("let x = 1;\nassert_eq!(format!(\"{x}\"), \"1\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_panic_message_does_not_inline() {
        // Before edition 2021 a lone `panic!("{x}")` literal is no format string.
        let a = canonicalize("let x = 1;\npanic!(\"{}\", x);\n");
        let b = canonicalize("let x = 1;\npanic!(\"{x}\");\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn a_width_argument_blocks_inlining() {
        let a = canonicalize("let x = 1;\nlet w = 4;\nprintln!(\"{:1$}\", x, w);\n");
        let b = canonicalize("let x = 1;\nlet w = 4;\nprintln!(\"{x:1$}\", w);\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn a_raw_identifier_argument_stays_positional() {
        let a = canonicalize("let r#type = 1;\nprintln!(\"{}\", r#type);\n");
        assert!(a.text.contains("\"{}\""), "{}", a.text);
    }

    #[test]
    fn inline_names_in_panic_and_assert_messages_rename() {
        let a = canonicalize("let x = 1;\npanic!(\"{x}\");\n");
        let b = canonicalize("let y = 1;\npanic!(\"{y}\");\n");
        assert_eq!(a.text, b.text);
        let a = canonicalize("let x = 1;\nassert!(x == 1, \"{x}\");\n");
        let b = canonicalize("let y = 1;\nassert!(y == 1, \"{y}\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_qualified_path_argument_stays_positional() {
        let a = canonicalize("println!(\"{}\", <S>::C);\n");
        let b = canonicalize("println!(\"{C}\");\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_attributed_argument_stays_positional() {
        let a = canonicalize("let x = 1;\nprintln!(\"{}\", #[cfg(unix)] x);\n");
        let b = canonicalize("let x = 1;\nprintln!(\"{x}\");\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn escaped_braces_survive_inlining() {
        let a = canonicalize("let x = 1;\nprintln!(\"{{}} {}\", x);\n");
        let b = canonicalize("let x = 1;\nprintln!(\"{{}} {x}\");\n");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_explicit_index_blocks_inlining() {
        let a = canonicalize("let x = 1;\nprintln!(\"{0} {}\", x);\n");
        let b = canonicalize("let x = 1;\nprintln!(\"{0} {x}\");\n");
        assert_ne!(a.text, b.text);
    }

    #[test]
    fn an_unused_argument_blocks_inlining() {
        let a = canonicalize("let x = 1;\nlet y = 2;\nprintln!(\"{}\", x, y);\n");
        let b = canonicalize("let x = 1;\nlet y = 2;\nprintln!(\"{x}\", y);\n");
        assert_ne!(a.text, b.text);
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
    fn a_label_and_a_lifetime_param_of_one_name_stay_distinct() {
        // rustc verified, `'a` as a label and `'a` as a lifetime parameter
        // are separate namespaces, the reference keeps the parameter and
        // the break keeps the label.
        let a = canonicalize(
            "fn f<'a>(v: bool, x: &'a u8) -> &'a u8 { 'a: loop { if v { continue 'a; } let y: &'a u8 = x; break 'a y; } }",
        );
        let b = canonicalize(
            "fn f<'z>(v: bool, x: &'z u8) -> &'z u8 { 'q: loop { if v { continue 'q; } let y: &'z u8 = x; break 'q y; } }",
        );
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_value_binding_does_not_capture_a_type_annotation() {
        // rustc verified, a parameter named `s` and the alias `s` are
        // separate namespaces, the return annotation keeps the alias.
        let a = canonicalize("type s = u8; fn f(s: u8) -> s { 1 }");
        let b = canonicalize("type zz = u8; fn f(q: u8) -> zz { 1 }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_value_binding_does_not_capture_a_trait_bound() {
        // rustc verified, the bound of the local struct resolves through
        // the type namespace even with a same-named parameter in scope.
        let a = canonicalize("trait t {} fn f(t: u8) { struct A<T: t>(T); }");
        let b = canonicalize("trait zz {} fn f(q: u8) { struct B<T: zz>(T); }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn an_expression_of_a_captured_name_keeps_the_value_namespace() {
        let a = canonicalize("type s = u8; fn f(s: u8) -> s { s }");
        let b = canonicalize("type zz = u8; fn f(q: u8) -> zz { q }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_value_use_after_a_bound_keeps_the_value_namespace() {
        let a = canonicalize("trait t {} fn f(t: u8) { struct A<T: t>(T); let _x = t; }");
        let b = canonicalize("trait zz {} fn f(q: u8) { struct B<T: zz>(T); let _x = q; }");
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn a_const_argument_expression_keeps_the_value_namespace() {
        // rustc verified, `s` names the alias in type position and the
        // const item inside the argument braces.
        let a = canonicalize(
            "type s = usize; const s: usize = 2; struct F<const N: usize, T> { n: core::marker::PhantomData<T> } const G: F<{ s }, s> = F { n: core::marker::PhantomData };",
        );
        let b = canonicalize(
            "type zz = usize; const q: usize = 2; struct H<const N: usize, T> { n: core::marker::PhantomData<T> } const R: H<{ q }, zz> = H { n: core::marker::PhantomData };",
        );
        assert_eq!(a.text, b.text);
    }

    fn token_after(text: &str, keyword: &str) -> String {
        let words: Vec<&str> = text
            .split_whitespace()
            .map(|w| w.trim_end_matches([')', ',', ';']))
            .collect();
        let i = words.iter().position(|w| *w == keyword).unwrap();
        words[i + 1].to_string()
    }

    #[test]
    fn a_use_alias_annotation_prints_the_declared_binder() {
        // The alias is printed once, the annotation must carry the very
        // canon the use statement declares, twin tests cannot see a split
        // because both sides split alike.
        let a = canonicalize("use a::T;\nfn f(x: T) {}");
        assert_eq!(token_after(&a.text, "as"), token_after(&a.text, ":"));
    }

    #[test]
    fn an_impl_trait_path_prints_the_declared_trait() {
        // A top level value sharing the trait name must not hijack the
        // impl trait path, a value-first lookup breaks the twin
        // identically on both sides, so the check is that the impl names
        // the declared trait.
        let a = canonicalize(
            "const t: u8 = 5; trait t { fn m(&self) -> u8; } struct W; impl t for W { fn m(&self) -> u8 { 1 } } fn g() { let _ = t; W.m(); }",
        );
        assert_eq!(token_after(&a.text, "trait"), token_after(&a.text, "impl"));
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
            "..=5",
            "a::B",
            "<T>::C",
            "const { 1 }",
            "x: u8",
            "..",
            "mac!()",
        ];
        for param in params {
            let with = canonicalize(&format!("let c = |#[expect(unused)] {param}| 0;"));
            let without = canonicalize(&format!("let c = |{param}| 0;"));
            assert_eq!(with.text, without.text, "param pattern {param}");
        }
    }

    #[test]
    fn inert_attrs_on_built_or_and_guard_patterns_merge() {
        // Only a tree built in code, a proc macro's output for one, attributes these.
        use quote::ToTokens as _;
        use syn::visit_mut::VisitMut;

        struct Attach;
        impl VisitMut for Attach {
            fn visit_pat_mut(&mut self, pat: &mut syn::Pat) {
                let attr: syn::Attribute = syn::parse_quote!(#[expect(unused)]);
                match pat {
                    syn::Pat::Or(p) => p.attrs.push(attr),
                    syn::Pat::Guard(p) => p.attrs.push(attr),
                    _ => {}
                }
                syn::visit_mut::visit_pat_mut(self, pat);
            }
        }
        for src in [
            "fn main() { match v { A | B => 1, _ => 2 }; }",
            "fn main() { match v { A if c => 1, _ => 2 }; }",
        ] {
            let plain: syn::File = syn::parse_str(src).unwrap();
            let mut marked = plain.clone();
            Attach.visit_file_mut(&mut marked);
            assert_ne!(
                marked.to_token_stream().to_string(),
                plain.to_token_stream().to_string()
            );
            assert_eq!(canon(marked).to_string(), canon(plain).to_string(), "{src}");
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

    /// Asserts each pair in `pairs` canonicalizes alike.
    fn assert_merge(pairs: &[(&str, &str)]) {
        for (a, b) in pairs {
            assert_eq!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
    }

    /// Asserts each pair in `pairs` canonicalizes apart.
    fn assert_split(pairs: &[(&str, &str)]) {
        for (a, b) in pairs {
            assert_ne!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
    }

    #[test]
    fn an_unneeded_binding_mut_merges() {
        assert_merge(&[
            (
                "let mut v = vec![1]; f(v.len());",
                "let v = vec![1]; f(v.len());",
            ),
            ("let mut v: u8 = 1; f(v);", "let v: u8 = 1; f(v);"),
            ("let h = |mut x: u8| x + 1;", "let h = |x: u8| x + 1;"),
            ("let h = |mut x| x + 1;", "let h = |x| x + 1;"),
            ("fn h(mut x: u8) -> u8 { x }", "fn h(x: u8) -> u8 { x }"),
            ("for mut x in v { f(x) }", "for x in v { f(x) }"),
            ("match v { mut x => f(x) }", "match v { x => f(x) }"),
            (
                "match v { mut x if x > 1 => f(x), _ => {} }",
                "match v { x if x > 1 => f(x), _ => {} }",
            ),
            ("if let mut x = v { f(x) }", "if let x = v { f(x) }"),
            (
                "let mut x @ 1..=3 = v else { return };",
                "let x @ 1..=3 = v else { return };",
            ),
            (
                "impl S { fn h(mut self) -> u8 { self.0 } }",
                "impl S { fn h(self) -> u8 { self.0 } }",
            ),
            (
                "impl S { fn h(mut self: Box<Self>) -> u8 { self.0 } }",
                "impl S { fn h(self: Box<Self>) -> u8 { self.0 } }",
            ),
        ]);
    }

    #[test]
    fn a_mut_that_changes_the_type_or_the_binding_mode_stays() {
        assert_split(&[
            (
                "impl S { fn h(&mut self) -> u8 { self.0 } }",
                "impl S { fn h(&self) -> u8 { self.0 } }",
            ),
            (
                "let Some(ref mut x) = o else { return };",
                "let Some(ref x) = o else { return };",
            ),
            ("let &mut x = r;", "let &x = r;"),
            ("let r = &mut v; f(r);", "let r = &v; f(r);"),
            ("static mut S: u8 = 1;", "static S: u8 = 1;"),
            ("let p: *mut u8 = q;", "let p: *const u8 = q;"),
            // Under a reference, a nested `mut` resets the binding mode to by-value before 2024.
            ("let [mut x] = &[0_u8]; f(x);", "let [x] = &[0_u8]; f(x);"),
            (
                "let (mut a, b) = g(); f(a, b);",
                "let (a, b) = g(); f(a, b);",
            ),
            (
                "if let Some(mut x) = o { f(x) }",
                "if let Some(x) = o { f(x) }",
            ),
            ("fn h((mut a, b): (u8, u8)) {}", "fn h((a, b): (u8, u8)) {}"),
            (
                "match o { Some(mut x) => f(x), None => {} }",
                "match o { Some(x) => f(x), None => {} }",
            ),
            (
                "let mut x @ Some(mut y) = o else { return };",
                "let mut x @ Some(y) = o else { return };",
            ),
        ]);
    }

    #[test]
    fn a_semicolon_after_a_block_statement_merges() {
        assert_merge(&[
            ("if c { f(); }; g();", "if c { f(); } g();"),
            ("match x { _ => f() }; g();", "match x { _ => f() } g();"),
            ("for i in v { f(i); }; g();", "for i in v { f(i); } g();"),
            ("while c { f(); }; g();", "while c { f(); } g();"),
            ("loop { break; }; g();", "loop { break; } g();"),
            ("{ f(); }; g();", "{ f(); } g();"),
            ("unsafe { f(); }; g();", "unsafe { f(); } g();"),
            (
                "let x = || { if c { f(); }; g() };",
                "let x = || { if c { f(); } g() };",
            ),
        ]);
    }

    #[test]
    fn a_semicolon_after_a_valued_tail_block_stays() {
        assert_split(&[
            (
                "let a = { if c { g() } else { h() } }; f(a);",
                "let a = { if c { g() } else { h() }; }; f(a);",
            ),
            (
                "fn k() -> u8 { match x { _ => 1 } }",
                "fn k() -> u8 { match x { _ => 1 }; }",
            ),
            (
                "let a = { if c { g() } else { h() } }; f(a);",
                "let a = { if c { g() } else { h() }; ; }; f(a);",
            ),
        ]);
    }

    #[test]
    fn a_unit_tail_semicolon_merges() {
        assert_merge(&[
            ("f(); g()", "f(); g();"),
            ("f(); println!(\"x\")", "f(); println!(\"x\");"),
            ("fn k() { f(); g() }", "fn k() { f(); g(); }"),
            ("fn k() -> () { f(); g() }", "fn k() { f(); g(); }"),
            (
                "impl S { fn k(&self) { g() } }",
                "impl S { fn k(&self) { g(); } }",
            ),
            (
                "trait T { fn k(&self) { g() } }",
                "trait T { fn k(&self) { g(); } }",
            ),
            (
                "for i in 0..3 { a[i] = b[i] } f();",
                "for i in 0..3 { a[i] = b[i]; } f();",
            ),
            ("while c { g() } f();", "while c { g(); } f();"),
            (
                "let v = loop { break 5 }; f(v);",
                "let v = loop { break 5; }; f(v);",
            ),
            (
                "if c { panic!(\"x\") } f();",
                "if c { panic!(\"x\"); } f();",
            ),
            (
                "if a { g() } else if b { h() } f();",
                "if a { g(); } else if b { h(); } f();",
            ),
            ("fn k() { f(); return; }", "fn k() { f() }"),
            (
                "fn k() { if c { return g(); } else { h() }; }",
                "fn k() { if c { g() } else { h() } }",
            ),
        ]);
    }

    #[test]
    fn a_tail_semicolon_where_the_value_is_used_stays() {
        assert_split(&[
            ("fn k() -> u8 { g() }", "fn k() -> u8 { g(); }"),
            ("let h = || { g() };", "let h = || { g(); };"),
            ("let a = async { g() };", "let a = async { g(); };"),
            ("let a = { g() }; f(a);", "let a = { g(); }; f(a);"),
            (
                "let a = if c { g() } else { h() };",
                "let a = if c { g(); } else { h(); };",
            ),
            (
                "let a = match x { _ => { g() } };",
                "let a = match x { _ => { g(); } };",
            ),
            ("let a = unsafe { g() };", "let a = unsafe { g(); };"),
        ]);
    }

    #[test]
    fn code_meant_to_fail_keeps_mut_and_semicolons() {
        let failing = |code: &str| super::canonicalize_failing(parse(code)).to_string();
        for (a, b) in [
            (
                "let mut v = Vec::new(); v.push(1);",
                "let v = Vec::new(); v.push(1);",
            ),
            ("impl S { fn h(mut self) {} }", "impl S { fn h(self) {} }"),
            ("match x { _ => 5 }; g();", "match x { _ => 5 } g();"),
            ("f(); 5", "f(); 5;"),
            ("for i in v { i }", "for i in v { i; }"),
        ] {
            assert_ne!(failing(a), failing(b), "{a} vs {b}");
            assert_eq!(canonicalize(a).text, canonicalize(b).text, "{a} vs {b}");
        }
        assert_eq!(
            failing("fn f(x: u8) -> u8 { return (x); }"),
            failing("fn g(y: u8) -> u8 { y }")
        );
    }

    #[test]
    fn an_associated_item_after_a_bare_qself_is_never_a_local() {
        assert_split(&[
            (
                "let new = 1; let s = <S>::new(); f(s, new);",
                "let default = 1; let s = <S>::default(); f(s, default);",
            ),
            (
                "let len = 2; f(<&str>::len(\"abc\") + len);",
                "let n = 2; f(<&str>::n(\"abc\") + n);",
            ),
            (
                "let x = 1; let s = <S>::x { a: x };",
                "let y = 1; let s = <S>::y { a: y };",
            ),
            (
                "let a = 1; let <S>::a(v) = g(a);",
                "let b = 1; let <S>::b(v) = g(b);",
            ),
            (
                "let a = 1; let <S>::a { v } = g(a);",
                "let b = 1; let <S>::b { v } = g(b);",
            ),
            (
                "let a = 1; let v: <<S>::a>::Out = g(a);",
                "let b = 1; let v: <<S>::b>::Out = g(b);",
            ),
        ]);
    }

    #[test]
    fn a_bare_qself_path_keeps_its_separator() {
        for code in [
            "let n = <&str>::len(\"abc\");",
            "let v = <Vec<u8>>::new();",
            "let v: <<S>::A>::B = g();",
            "let <S>::A(v) = g();",
        ] {
            let text = canonicalize(code).text;
            assert!(
                syn::parse_str::<syn::File>(&text).is_ok(),
                "{code} → {text}"
            );
        }
    }

    #[test]
    fn binders_around_a_bare_qself_path_still_rename() {
        assert_merge(&[(
            "let alpha = 1; let s = <S>::new(alpha); f(<S as T>::g(alpha));",
            "let beta = 1; let s = <S>::new(beta); f(<S as T>::g(beta));",
        )]);
    }
}
