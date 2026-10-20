//! Public contextual resolution of local helper calls through original views.

#[path = "support/spans.rs"]
mod spans;

use spans::reset_spans;
fn context(file: &syn::File) -> syn_canon::SourceContext<'_> {
    syn_canon::SourceContext::new(core::iter::once((&[][..], file)))
}
fn function<'a>(file: &'a syn::File, module: Option<&str>, name: &str) -> &'a syn::ItemFn {
    let inner = match module {
        None => &file.items[..],
        Some(module) => file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Mod(mod_item) if mod_item.ident == module => {
                    mod_item.content.as_ref().map(|(_, inner)| inner)
                }
                _ => None,
            })
            .unwrap(),
    };
    inner
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap()
}

/// The borrowed view of one original function in its enclosing context.
fn view(source: &str, module: Option<&str>, name: &str) -> syn_canon::CanonicalForm {
    let file: syn::File = syn::parse_str(source).unwrap();
    let context = context(&file);
    let function = function(&file, module, name);
    context
        .function(&function.sig, &function.block)
        .unwrap()
        .canonicalize()
}

fn pair(source: &str, module: Option<&str>) {
    assert_eq!(
        view(source, module, "first"),
        view(source, module, "same"),
        "the helper calls schedule under their original context"
    );
}

#[test]
fn enclosing_aliases_resolve_helper_parameter_widths() {
    pair(
        "mod m { type W = u32; fn low(x: W) -> W { x & 15u32 } pub fn first(input: W) -> (W, W) { let a = low(input); let b = low(input ^ 1u32); (a, b) } pub fn same(input: W) -> (W, W) { let b = low(input ^ 1u32); let a = low(input); (a, b) } }",
        Some("m"),
    );
    assert_ne!(
        view(
            "mod m { type W = u32; fn low(x: W) -> W { x & 15u32 } pub fn f(input: W) -> W { low(input) } }",
            Some("m"),
            "f"
        ),
        view(
            "mod m { type W = u64; fn low(x: W) -> W { x & 15u64 } pub fn f(input: W) -> W { low(input) } }",
            Some("m"),
            "f"
        ),
        "the alias keeps the helper's resolved width"
    );
}

#[test]
fn imported_helpers_resolve_through_module_boundaries() {
    pair(
        "mod inner { pub fn low(x: u32) -> u32 { x & 15u32 } } mod outer { use super::inner::low; pub fn first(input: u32) -> (u32, u32) { let a = low(input); let b = low(input ^ 1u32); (a, b) } pub fn same(input: u32) -> (u32, u32) { let b = low(input ^ 1u32); let a = low(input); (a, b) } }",
        Some("outer"),
    );
    pair(
        "mod inner { pub fn low(x: u32) -> u32 { x & 15u32 } } mod mid { pub use super::inner::low; } mod outer { use super::mid::low; pub fn first(input: u32) -> (u32, u32) { let a = low(input); let b = low(input ^ 1u32); (a, b) } pub fn same(input: u32) -> (u32, u32) { let b = low(input ^ 1u32); let a = low(input); (a, b) } }",
        Some("outer"),
    );
}

#[test]
fn qualified_calls_resolve_their_target_module() {
    assert_eq!(
        view(
            "mod a { pub fn low(x: u32) -> u32 { x & 15u32 } } mod b { pub fn f(input: u32) -> u32 { crate::a::low(input) } }",
            Some("b"),
            "f"
        ),
        view(
            "mod a { pub fn low(x: u32) -> u32 { x & 15u32 } } mod b { use super::a::low; pub fn f(input: u32) -> u32 { low(input) } }",
            Some("b"),
            "f"
        ),
        "the qualified call and the imported call share one resolved origin"
    );
    assert_ne!(
        view(
            "mod a { pub fn low(x: u32) -> u32 { x & 15u32 } } mod b { pub fn low(x: u64) -> u64 { x & 15u64 } } mod c { pub fn f(input: u32) -> u32 { crate::a::low(input) } pub fn g(input: u64) -> u64 { crate::b::low(input) } }",
            Some("c"),
            "f"
        ),
        view(
            "mod a { pub fn low(x: u32) -> u32 { x & 15u32 } } mod b { pub fn low(x: u64) -> u64 { x & 15u64 } } mod c { pub fn f(input: u32) -> u32 { crate::a::low(input) } pub fn g(input: u64) -> u64 { crate::b::low(input) } }",
            Some("c"),
            "g"
        ),
        "each module keeps its own helper identity"
    );
}

#[test]
fn repeated_parent_module_segments_keep_the_helper_origin() {
    let source = "pub fn low(p:bool)->bool{p}mod one{pub mod two{pub fn f(p:bool)->(bool,bool){let a=super::super::low(p);let b=super::super::low(!p);(a,b)}}}";
    let reordered = source.replace(
        "let a=super::super::low(p);let b=super::super::low(!p);",
        "let b=super::super::low(!p);let a=super::super::low(p);",
    );
    assert_eq!(
        syn_canon::canonicalize(syn::parse_str(source).unwrap()),
        syn_canon::canonicalize(syn::parse_str(&reordered).unwrap())
    );
}

#[test]
fn interior_parent_segments_cannot_supply_a_helper_proof() {
    let source = "mod one{}fn low(p:bool)->bool{p}fn f(p:bool)->(bool,bool){let a=self::one::super::low(p);let b=self::one::super::low(!p);(a,b)}";
    let reordered = source.replace(
        "let a=self::one::super::low(p);let b=self::one::super::low(!p);",
        "let b=self::one::super::low(!p);let a=self::one::super::low(p);",
    );
    assert_ne!(
        syn_canon::canonicalize(syn::parse_str(source).unwrap()),
        syn_canon::canonicalize(syn::parse_str(&reordered).unwrap())
    );
}

#[test]
fn original_macro_scope_keeps_helper_calls_ordered() {
    let source = "fn low(p:bool)->bool{p}fn first(p:bool)->(bool,bool){macro_rules! unused{()=>{}}let a=low(p);let b=low(!p);(a,b)}fn same(p:bool)->(bool,bool){macro_rules! unused{()=>{}}let b=low(!p);let a=low(p);(a,b)}";
    assert_ne!(view(source, None, "first"), view(source, None, "same"));
}

#[test]
fn original_external_crate_scope_keeps_helper_calls_ordered() {
    let source = "fn low(p:bool)->bool{p}fn first(p:bool)->(bool,bool){extern crate core;let a=low(p);let b=low(!p);(a,b)}fn same(p:bool)->(bool,bool){extern crate core;let b=low(!p);let a=low(p);(a,b)}";
    assert_ne!(view(source, None, "first"), view(source, None, "same"));
}

#[test]
fn raw_identifiers_resolve_their_raw_spelling() {
    pair(
        "mod m { fn r#type(x: u32) -> u32 { x & 15u32 } pub fn first(input: u32) -> (u32, u32) { let a = r#type(input); let b = r#type(input ^ 1u32); (a, b) } pub fn same(input: u32) -> (u32, u32) { let b = r#type(input ^ 1u32); let a = r#type(input); (a, b) } }",
        Some("m"),
    );
}

#[test]
fn closer_bindings_mask_the_helper_for_their_view() {
    let shadowed =
        "fn low(x: u32) -> u32 { x & 15u32 } fn f(x: u32) -> u32 { let low = |v: u32| v; low(x) }";
    let shadowed_changed =
        "fn low(x: u32) -> u32 { x >> 4 } fn f(x: u32) -> u32 { let low = |v: u32| v; low(x) }";
    assert_eq!(
        view(shadowed, None, "f"),
        view(shadowed_changed, None, "f"),
        "the masked helper body is invisible to the caller"
    );
    let total = "fn low(x: u32) -> u32 { x & 15u32 } fn first(x: u32) -> (u32, u32) { let a = low(x); let b = low(x ^ 1u32); (a, b) } fn same(x: u32) -> (u32, u32) { let b = low(x ^ 1u32); let a = low(x); (a, b) }";
    pair(total, None);
    let partial = total.replace("x & 15u32", "x / 0u32");
    assert_ne!(view(&partial, None, "first"), view(&partial, None, "same"));
    // A parameter named like the helper selects the parameter.
    let parameter = "fn low(x: u32) -> u32 { x & 15u32 } fn f(low: u32) -> (u32, u32) { let a = low; let b = low ^ 1u32; (a, b) }";
    let parameter_swapped = "fn low(x: u32) -> u32 { x & 15u32 } fn f(low: u32) -> (u32, u32) { let b = low ^ 1u32; let a = low; (a, b) }";
    assert_eq!(
        view(parameter, None, "f"),
        view(parameter_swapped, None, "f")
    );
    assert_eq!(
        view(parameter, None, "f"),
        view(
            "fn low(x: u32) -> u32 { x >> 4 } fn f(low: u32) -> (u32, u32) { let a = low; let b = low ^ 1u32; (a, b) }",
            None,
            "f"
        ),
        "the parameter shadows the helper entirely"
    );
}

#[test]
fn primitive_alias_shadowing_resolves_helper_widths() {
    pair(
        "mod m { type u32 = ::core::primitive::u32; fn low(x: u32) -> u32 { x & 15u32 } pub fn first(input: u32) -> (u32, u32) { let a = low(input); let b = low(input ^ 1u32); (a, b) } pub fn same(input: u32) -> (u32, u32) { let b = low(input ^ 1u32); let a = low(input); (a, b) } }",
        Some("m"),
    );
    assert_ne!(
        view(
            "mod m { type u32 = u64; fn low(x: u32) -> u32 { x & 15u64 } pub fn f(input: u32) -> u32 { low(input) } }",
            Some("m"),
            "f"
        ),
        view(
            "mod m { fn low(x: u32) -> u32 { x & 15u32 } pub fn f(input: u32) -> u32 { low(input) } }",
            Some("m"),
            "f"
        ),
        "a shadowing alias changes the helper's resolved width"
    );
}

#[test]
fn a_view_selects_the_exact_original_signature_and_body() {
    let source = "mod a { pub fn f(input: u32) -> u32 { input & 15u32 } } mod b { pub fn f(input: u32) -> u32 { input >> 4 } }";
    let file: syn::File = syn::parse_str(source).unwrap();
    let context = context(&file);
    let (first, second) = (
        function(&file, Some("a"), "f"),
        function(&file, Some("b"), "f"),
    );
    assert_ne!(
        context
            .function(&first.sig, &first.block)
            .unwrap()
            .canonicalize(),
        context
            .function(&second.sig, &second.block)
            .unwrap()
            .canonicalize(),
        "identical signatures with different bodies keep distinct views"
    );
    assert!(
        context.function(&first.sig, &second.block).is_none(),
        "a signature paired with a foreign body is not a declaration"
    );
    assert!(
        context.function(&second.sig, &first.block).is_none(),
        "the reverse cross-wiring is not a declaration either"
    );
}

#[test]
fn the_view_keeps_helper_origin_across_reordering() {
    let source = "mod m { fn low(x: u32) -> u32 { x & 15u32 } pub fn first(input: u32) -> (u32, u32) { let a = low(input); let b = low(input ^ 1u32); (a, b) } pub fn same(input: u32) -> (u32, u32) { let b = low(input ^ 1u32); let a = low(input); (a, b) } pub fn changed(input: u32) -> (u32, u32) { let a = low(input); let b = low(input); (a, b) } }";
    pair(source, Some("m"));
    assert_ne!(
        view(source, Some("m"), "first"),
        view(source, Some("m"), "changed"),
        "call multiplicity is preserved, with no merging"
    );
}

#[test]
fn detached_reporting_keeps_unresolved_helper_order() {
    let detached = |body: &str| syn_canon::canonicalize(syn::parse_str(body).unwrap());
    assert_ne!(
        detached(
            "pub fn first(input: u32) -> (u32, u32) { let a = low(input); let b = low(input ^ 1u32); (a, b) }"
        ),
        detached(
            "pub fn same(input: u32) -> (u32, u32) { let b = low(input ^ 1u32); let a = low(input); (a, b) }"
        ),
        "without the helper in scope the calls stay unresolved and ordered"
    );
}

#[test]
fn constructed_helper_origins_ignore_shared_spans() {
    let helpers = "fn low(x:u32)->u32{x&15u32}fn high(x:u32)->u32{x>>4}";
    let source = format!(
        "{helpers}fn first(x:u32)->(u32,u32){{let a=low(x);let b=high(x);(a,b)}}fn same(x:u32)->(u32,u32){{let b=high(x);let a=low(x);(a,b)}}fn changed(x:u32)->(u32,u32){{let a=low(x);let b=low(x);(a,b)}}"
    );
    let file: syn::File = syn::parse2(reset_spans(source.parse().unwrap())).unwrap();
    let context = context(&file);
    let view = |name: &str| {
        let declaration = function(&file, None, name);
        context
            .function(&declaration.sig, &declaration.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(view("first"), view("same"));
    assert_ne!(view("first"), view("changed"));
    let original = function(&file, None, "first");
    let cloned = original.block.clone();
    assert!(context.function(&original.sig, &cloned).is_none());
}

#[test]
fn ancestor_observers_keep_original_helper_call_order() {
    let helpers = "fn one(x:u32)->u32{x&15u32}fn two(x:u32)->u32{x>>4}";
    let first = format!(
        "#[m6_observer::observe(\"scope\")]mod m{{{helpers}pub fn f(x:u32)->(u32,u32){{let a=one(x);let b=two(x);(a,b)}}}}"
    );
    let same = format!(
        "#[m6_observer::observe(\"scope\")]mod m{{{helpers}pub fn f(x:u32)->(u32,u32){{let b=two(x);let a=one(x);(a,b)}}}}"
    );
    assert_ne!(view(&first, Some("m"), "f"), view(&same, Some("m"), "f"));
    assert_ne!(
        syn_canon::canonicalize(syn::parse_str(&first).unwrap()),
        syn_canon::canonicalize(syn::parse_str(&same).unwrap())
    );
}

#[test]
fn same_spelled_module_helpers_keep_distinct_origins() {
    let source = "mod a{pub fn low(x:u32)->u32{x&15u32}}mod b{pub fn low(x:u32)->u32{x>>4}}mod c{pub fn f(x:u32)->u32{crate::a::low(x)}pub fn g(x:u32)->u32{crate::b::low(x)}}";
    assert_ne!(view(source, Some("c"), "f"), view(source, Some("c"), "g"));
}

#[test]
fn caller_local_module_aliases_keep_the_resolved_schedule() {
    pair(
        "mod a{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}fn first(x:u32)->(u32,u32){use crate::a as m;let a=m::low(x);let b=m::high(x);(a,b)}fn same(x:u32)->(u32,u32){use crate::a as m;let b=m::high(x);let a=m::low(x);(a,b)}",
        None,
    );
}

#[test]
fn caller_local_function_imports_keep_the_resolved_schedule() {
    pair(
        "mod a{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}fn first(x:u32)->(u32,u32){use crate::a::{low as h,high as k};let a=h(x);let b=k(x);(a,b)}fn same(x:u32)->(u32,u32){use crate::a::{low as h,high as k};let b=k(x);let a=h(x);(a,b)}",
        None,
    );
}

#[test]
fn caller_imports_preserve_separate_value_and_module_namespaces() {
    pair(
        "mod a{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}mod other{pub fn a(x:u32)->u32{x}}fn first(x:u32)->(u32,u32){use crate::other::a;let a=a::low(x);let b=a::high(x);(a,b)}fn same(x:u32)->(u32,u32){use crate::other::a;let b=a::high(x);let a=a::low(x);(a,b)}",
        None,
    );
}

#[test]
fn block_local_module_helpers_keep_the_resolved_schedule() {
    let first = "fn f(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let a=m::low(x);let b=m::high(x);(a,b)}";
    let same = "fn f(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let b=m::high(x);let a=m::low(x);(a,b)}";
    assert_eq!(view(first, None, "f"), view(same, None, "f"));
    assert_eq!(
        syn_canon::canonicalize(syn::parse_str(first).unwrap()),
        syn_canon::canonicalize(syn::parse_str(same).unwrap())
    );
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(first).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(same).unwrap())
    );
}

#[test]
fn caller_local_globs_cannot_select_a_farther_helper_module() {
    let prefix = "mod a{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}mod other{pub mod a{pub fn low(_:u32)->u32{panic!(\"low\")}pub fn high(_:u32)->u32{panic!(\"high\")}}}";
    let first = format!(
        "{prefix}fn f(x:u32)->(u32,u32){{use crate::other::*;let a=a::low(x);let b=a::high(x);(a,b)}}"
    );
    let same = format!(
        "{prefix}fn f(x:u32)->(u32,u32){{use crate::other::*;let b=a::high(x);let a=a::low(x);(a,b)}}"
    );
    assert_ne!(view(&first, None, "f"), view(&same, None, "f"));
}

#[test]
fn tuple_constructors_mask_same_spelled_helper_values() {
    let prefix = "fn low(x:u32)->u32{x&15u32}";
    let first = format!(
        "{prefix}fn f(x:u32)->(u32,u32){{struct low(u32);impl Drop for low{{fn drop(&mut self){{println!(\"drop {{}}\",self.0);}}}}let a=low(x);let b=low(x^1u32);(a.0,b.0)}}"
    );
    let same = format!(
        "{prefix}fn f(x:u32)->(u32,u32){{struct low(u32);impl Drop for low{{fn drop(&mut self){{println!(\"drop {{}}\",self.0);}}}}let b=low(x^1u32);let a=low(x);(a.0,b.0)}}"
    );
    assert_ne!(view(&first, None, "f"), view(&same, None, "f"));
}

#[test]
fn repeated_local_modules_keep_separate_lexical_origins() {
    pair(
        "fn first(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let a=m::low(x);let b=m::high(x);(a,b)}fn same(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let b=m::high(x);let a=m::low(x);(a,b)}",
        None,
    );
}

#[test]
fn nearer_local_modules_mask_root_modules() {
    pair(
        "mod m{pub fn low(_:u32)->u32{panic!()}pub fn high(_:u32)->u32{panic!()}}fn first(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let a=m::low(x);let b=m::high(x);(a,b)}fn same(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}let b=m::high(x);let a=m::low(x);(a,b)}",
        None,
    );
}

#[test]
fn local_module_imports_keep_their_original_scope() {
    pair(
        "fn first(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}use m::{low as h,high as k};let a=h(x);let b=k(x);(a,b)}fn same(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}use m::{low as h,high as k};let b=k(x);let a=h(x);(a,b)}",
        None,
    );
}

#[test]
fn local_module_super_selects_the_enclosing_module() {
    pair(
        "fn base(x:u32)->u32{x&15u32}fn first(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{super::base(x)}pub fn high(x:u32)->u32{x>>4}}let a=m::low(x);let b=m::high(x);(a,b)}fn same(x:u32)->(u32,u32){mod m{pub fn low(x:u32)->u32{super::base(x)}pub fn high(x:u32)->u32{x>>4}}let b=m::high(x);let a=m::low(x);(a,b)}",
        None,
    );
}

#[test]
fn qualified_aliases_prove_helper_signature_widths() {
    pair(
        "mod a{pub type W=u32;}fn low(x:a::W)->a::W{x&15u32}fn high(x:a::W)->a::W{x>>4}fn first(x:u32)->(u32,u32){let a=low(x);let b=high(x);(a,b)}fn same(x:u32)->(u32,u32){let b=high(x);let a=low(x);(a,b)}",
        None,
    );
}

#[test]
fn deep_local_modules_keep_the_nearer_callee() {
    let prefix = "mod m{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}";
    let nested = |body: &str| {
        format!(
            "{prefix}fn f(x:u32)->(u32,u32){{{}mod m{{pub fn low(_:u32)->u32{{panic!(\"low\")}}pub fn high(_:u32)->u32{{panic!(\"high\")}}}}{body}{}}}",
            "{".repeat(40),
            "}".repeat(40)
        )
    };
    let first = nested("let a=m::low(x);let b=m::high(x);(a,b)");
    let same = nested("let b=m::high(x);let a=m::low(x);(a,b)");
    assert_ne!(view(&first, None, "f"), view(&same, None, "f"));
}

#[test]
fn deep_local_modules_keep_the_total_schedule() {
    let nested = |body: &str| {
        format!(
            "fn f(x:u32)->(u32,u32){{{}mod m{{pub fn low(x:u32)->u32{{x&15u32}}pub fn high(x:u32)->u32{{x>>4}}}}{body}{}}}",
            "{".repeat(40),
            "}".repeat(40)
        )
    };
    let first = nested("let a=m::low(x);let b=m::high(x);(a,b)");
    let same = nested("let b=m::high(x);let a=m::low(x);(a,b)");
    assert_eq!(view(&first, None, "f"), view(&same, None, "f"));
}

#[test]
fn grouped_nested_imports_keep_resolved_helper_origins() {
    pair(
        "mod a{pub mod inner{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}}fn first(x:u32)->(u32,u32){use crate::a::{inner::{low as h,high as k},self as aa};let a=h(x);let b=k(x);(a,b)}fn same(x:u32)->(u32,u32){use crate::a::{inner::{low as h,high as k},self as aa};let b=k(x);let a=h(x);(a,b)}",
        None,
    );
}

#[test]
fn parenthesized_callees_keep_original_block_context() {
    pair(
        "fn low(x:u32)->u32{x&15u32}fn high(x:u32)->u32{x>>4}fn first(x:u32)->(u32,u32){let a=(low)(x);let b=(high)(x);(a,b)}fn same(x:u32)->(u32,u32){let b=(high)(x);let a=(low)(x);(a,b)}",
        None,
    );
}

#[test]
fn self_module_imports_keep_the_enclosing_namespace() {
    pair(
        "mod m{use self as mm;fn low(x:u32)->u32{x&15u32}fn high(x:u32)->u32{x>>4}fn first(x:u32)->(u32,u32){let a=mm::low(x);let b=mm::high(x);(a,b)}fn same(x:u32)->(u32,u32){let b=mm::high(x);let a=mm::low(x);(a,b)}}",
        Some("m"),
    );
}

#[test]
fn local_module_children_keep_independent_namespaces() {
    let build = |body: &str| {
        format!(
            "fn f(x:u32)->(u32,u32){{mod m{{pub mod child{{pub fn low(x:u32)->u32{{x&15u32}}pub fn high(x:u32)->u32{{x>>4}}}}}}{body}}}"
        )
    };
    let first = build("let a=m::child::low(x);let b=m::child::high(x);(a,b)");
    let same = build("let b=m::child::high(x);let a=m::child::low(x);(a,b)");
    assert_eq!(view(&first, None, "f"), view(&same, None, "f"));
}

#[test]
fn lexical_type_heads_mask_farther_primitive_aliases() {
    let prefix = "mod a{pub type W=u32;}";
    let build = |body: &str| {
        format!("{prefix}fn f(x:u32)->(u32,u32){{struct a;type W=a::W;fn h(x:W)->W{{x}}{body}}}")
    };
    assert_ne!(
        view(&build("let a=h(x);let b=h(x^1u32);(a,b)"), None, "f"),
        view(&build("let b=h(x^1u32);let a=h(x);(a,b)"), None, "f")
    );
}

#[test]
fn reexported_modules_and_external_primitives_keep_helper_proofs() {
    pair(
        "mod a{pub fn low(x:u32)->u32{x&15u32}pub fn high(x:u32)->u32{x>>4}}mod bridge{pub use crate::a as m;}fn first(x:u32)->(u32,u32){use bridge::m::{low,high};let a=low(x);let b=high(x);(a,b)}fn same(x:u32)->(u32,u32){use bridge::m::{low,high};let b=high(x);let a=low(x);(a,b)}",
        None,
    );
    pair(
        "mod a{pub use core::primitive::u32 as W;}type T=a::W;fn low(x:T)->T{x&15u32}fn high(x:T)->T{x>>4}fn first(x:u32)->(u32,u32){let a=low(x);let b=high(x);(a,b)}fn same(x:u32)->(u32,u32){let b=high(x);let a=low(x);(a,b)}",
        None,
    );
    pair(
        "fn base(x:u32)->u32{x&15u32}fn low(x:u32)->u32{self::base(x)}fn high(x:u32)->u32{x>>4}fn first(x:u32)->(u32,u32){let a=low(x);let b=high(x);(a,b)}fn same(x:u32)->(u32,u32){let b=high(x);let a=low(x);(a,b)}",
        None,
    );
}

#[test]
fn incomplete_public_namespace_heads_cannot_supply_a_helper_proof() {
    for prefix in [
        "mod a{pub fn h(x:u32)->u32{x}}use a::h;",
        "mod a{pub struct h{pub x:u32}}use a::h;",
        "mod a{pub fn h(x:u32)->u32{x}}mod b{pub struct h{pub x:u32}}mod m{pub use crate::a::h;pub use crate::b::h;}",
    ] {
        let callee = if prefix.contains("mod m") {
            "m::h"
        } else {
            "h::missing"
        };
        let a = format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let a={callee}(x);let b={callee}(x^1u32);(a,b)}}"
        );
        let b = format!(
            "{prefix}fn f(x:u32)->(u32,u32){{let b={callee}(x^1u32);let a={callee}(x);(a,b)}}"
        );
        assert_ne!(view(&a, None, "f"), view(&b, None, "f"));
    }
}

#[test]
fn public_alias_ast_gaps_keep_helper_calls_ordered() {
    for deep in [false, true] {
        let build = |body: &str| {
            let mut file: syn::File = syn::parse_str(&format!(
                "type T=u32;fn h(x:T)->T{{x}}fn f(x:u32)->(u32,u32){{{body}}}"
            ))
            .unwrap();
            let syn::Item::Type(alias) = &mut file.items[0] else {
                unreachable!("the fixture's first item is a type alias")
            };
            if deep {
                let mut ty = Box::new(syn::parse_str::<syn::Type>("u32").unwrap());
                for _ in 0..65 {
                    ty = Box::new(syn::Type::Paren(syn::TypeParen {
                        attrs: Vec::new(),
                        paren_token: syn::token::Paren::default(),
                        elem: ty,
                    }));
                }
                alias.ty = ty;
            } else {
                let syn::Type::Path(path) = alias.ty.as_mut() else {
                    unreachable!("the alias target is a path")
                };
                path.path.segments.clear();
            }
            let context = context(&file);
            let f = function(&file, None, "f");
            context.function(&f.sig, &f.block).unwrap().canonicalize()
        };
        assert_ne!(
            build("let a=h(x);let b=h(x^1u32);(a,b)"),
            build("let b=h(x^1u32);let a=h(x);(a,b)")
        );
    }
}

#[test]
fn transparent_helper_type_exhaustion_keeps_the_original_call_order() {
    let wrapped = format!("{}u32{}", "(".repeat(65), ")".repeat(65));
    let helpers = format!("fn h(x:{wrapped})->u32{{x}}");
    let a = format!("{helpers}fn f(x:u32)->(u32,u32){{let a=h(x);let b=h(x^1u32);(a,b)}}");
    let b = format!("{helpers}fn f(x:u32)->(u32,u32){{let b=h(x^1u32);let a=h(x);(a,b)}}");
    assert_ne!(view(&a, None, "f"), view(&b, None, "f"));
}

#[test]
fn incomplete_original_primitive_namespaces_keep_closed_calls_ordered() {
    let build = |body: &str| {
        format!("extern crate unknown;fn h(x:bool)->bool{{x}}fn f()->(bool,bool){{{body}}}")
    };
    assert_ne!(
        view(&build("let a=h(true);let b=h(false);(a,b)"), None, "f"),
        view(&build("let b=h(false);let a=h(true);(a,b)"), None, "f")
    );
}

#[test]
fn qualified_parenthesized_aliases_keep_their_primitive_proof() {
    pair(
        "mod a{pub type W=(u32);}fn low(x:a::W)->a::W{x&15u32}fn high(x:a::W)->a::W{x>>4}fn first(x:u32)->(u32,u32){let a=low(x);let b=high(x);(a,b)}fn same(x:u32)->(u32,u32){let b=high(x);let a=low(x);(a,b)}",
        None,
    );
}

#[test]
fn unresolved_and_generic_qualified_aliases_keep_calls_ordered() {
    for alias in ["pub type W<T>=T;", "pub type W=missing::W;"] {
        let build = |body: &str| {
            format!("mod m{{{alias}}}fn h(x:m::W)->m::W{{x}}fn f(x:u32)->(u32,u32){{{body}}}")
        };
        assert_ne!(
            view(&build("let a=h(x);let b=h(x^1u32);(a,b)"), None, "f"),
            view(&build("let b=h(x^1u32);let a=h(x);(a,b)"), None, "f")
        );
    }
}

#[test]
fn reexport_namespace_ambiguity_cannot_supply_an_imported_helper() {
    for prefix in [
        "mod a{pub fn h(x:u32)->u32{x}}use a::h;use h::missing as call;",
        "mod a{pub fn h(x:u32)->u32{x}}mod b{pub struct h{pub x:u32}}mod m{pub use crate::a::h;pub use crate::b::h;}use m::h as call;",
    ] {
        let build = |body: &str| format!("{prefix}fn f(x:u32)->(u32,u32){{{body}}}");
        assert_ne!(
            view(&build("let a=call(x);let b=call(x^1u32);(a,b)"), None, "f"),
            view(&build("let b=call(x^1u32);let a=call(x);(a,b)"), None, "f")
        );
    }
}

#[test]
fn imported_types_do_not_replace_same_spelled_local_helpers() {
    pair(
        "mod source{pub struct helper{pub value:u32}}fn first(x:u32)->(u32,u32){use source::helper;fn helper(x:u32)->u32{x&15u32}let a=helper(x);let b=helper(x^1u32);(a,b)}fn same(x:u32)->(u32,u32){use source::helper;fn helper(x:u32)->u32{x&15u32}let b=helper(x^1u32);let a=helper(x);(a,b)}",
        None,
    );
}

#[test]
fn user_module_names_cannot_collide_with_private_lexical_routes() {
    pair(
        "mod __dejadoc_local_module_0{}fn first(x:u32)->(u32,u32){mod m{pub fn h(x:u32)->u32{x&15u32}}let a=m::h(x);let b=m::h(x^1u32);(a,b)}fn same(x:u32)->(u32,u32){mod m{pub fn h(x:u32)->u32{x&15u32}}let b=m::h(x^1u32);let a=m::h(x);(a,b)}",
        None,
    );
}

#[test]
fn exhausted_grouped_import_evidence_keeps_calls_ordered() {
    let grouped = format!("source::{}h{}", "{".repeat(65), "}".repeat(65));
    let build = |body: &str| {
        format!(
            "mod source{{pub fn h(x:u32)->u32{{x&15u32}}}}fn f(x:u32)->(u32,u32){{use {grouped};{body}}}"
        )
    };
    assert_ne!(
        view(&build("let a=h(x);let b=h(x^1u32);(a,b)"), None, "f"),
        view(&build("let b=h(x^1u32);let a=h(x);(a,b)"), None, "f")
    );
}

#[test]
fn malformed_empty_self_imports_cannot_supply_a_helper_origin() {
    let build = |body: &str| format!("fn f(x:u32)->(u32,u32){{use {{self}};{body}}}");
    assert_ne!(
        view(&build("let a=call(x);let b=call(x^1u32);(a,b)"), None, "f"),
        view(&build("let b=call(x^1u32);let a=call(x);(a,b)"), None, "f")
    );
}
