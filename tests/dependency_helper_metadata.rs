//! Original helper provenance through syntax movement and cloning.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn view(source: &str) -> syn_canon::CanonicalForm {
    let file: syn::File = syn::parse_str(source).unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &file)));
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "f" => Some(function),
            _ => None,
        })
        .unwrap();
    context
        .function(&function.sig, &function.block)
        .unwrap()
        .canonicalize()
}

fn ordered(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "standalone helper provenance");
    assert_ne!(view(left), view(right), "borrowed helper provenance");
}

fn scheduled(left: &str, right: &str) {
    assert_eq!(form(left), form(right), "standalone helper provenance");
    assert_eq!(view(left), view(right), "borrowed helper provenance");
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(right).unwrap()),
        "the caller needs helper scheduling"
    );
}

const FORWARD: &str = "let a=low(x);let b=low(x^1u32);(a,b)";
const REVERSE: &str = "let b=low(x^1u32);let a=low(x);(a,b)";

fn local_module(body: &str, root: &str, local: &str) -> String {
    format!(
        "mod m{{pub mod inner{{pub fn low(x:u32)->u32{{{root}}}}}}}fn f(x:u32)->(u32,u32){{mod m{{use self::inner::low;mod inner{{pub fn low(x:u32)->u32{{{local}}}}}pub fn g(x:u32)->(u32,u32){{{body}}}}}m::g(x)}}"
    )
}

#[test]
fn block_local_module_import_keeps_its_rejected_origin() {
    ordered(
        &local_module(FORWARD, "x&15u32", "panic!(\"{x}\")"),
        &local_module(REVERSE, "x&15u32", "panic!(\"{x}\")"),
    );
}

#[test]
fn block_local_module_import_keeps_its_total_origin() {
    scheduled(
        &local_module(FORWARD, "panic!(\"{x}\")", "x&15u32"),
        &local_module(REVERSE, "panic!(\"{x}\")", "x&15u32"),
    );
}

fn else_if(body: &str, target: &str) -> String {
    format!(
        "fn good(x:u32)->u32{{x&15u32}}fn bad(x:u32)->u32{{panic!(\"{{x}}\")}}use crate::good as low;fn f(x:u32)->(u32,u32){{if x==0u32{{(0u32,0u32)}}else{{if x==1u32{{use crate::{target} as low;{body}}}else{{(0u32,0u32)}}}}}}"
    )
}

#[test]
fn collapsed_else_if_keeps_its_rejected_import_scope() {
    ordered(&else_if(FORWARD, "bad"), &else_if(REVERSE, "bad"));
}

#[test]
fn collapsed_else_if_keeps_its_total_import_scope() {
    scheduled(&else_if(FORWARD, "good"), &else_if(REVERSE, "good"));
}

#[test]
fn moving_items_and_empty_statements_keep_local_declarations() {
    for helper in ["x&15u32", "panic!(\"{x}\")"] {
        let source = |body: &str| {
            format!(
                "fn low(x:u32)->u32{{x&7u32}}fn f(x:u32)->(u32,u32){{;;let unused=();;fn low(x:u32)->u32{{{helper}}};mod vacant{{}};fn empty(){{}};{body}}}"
            )
        };
        if helper == "x&15u32" {
            scheduled(&source(FORWARD), &source(REVERSE));
        } else {
            ordered(&source(FORWARD), &source(REVERSE));
        }
    }
}

#[test]
fn tail_returns_and_parentheses_keep_original_call_scopes() {
    let source = |body: &str| {
        format!(
            "fn low(x:u32)->u32{{x&15u32}}fn f(x:u32)->(u32,u32){{if x==0u32{{return (0u32,0u32)}}else{{{body}}}}}"
        )
    };
    scheduled(
        &source("let a=(low)(x);let b=(low)(x^1u32);return (a,b);"),
        &source("let b=(low)(x^1u32);let a=(low)(x);return (a,b);"),
    );
}

#[test]
fn unwrapped_arms_closures_and_return_values_keep_nested_scopes() {
    for target in ["good", "bad"] {
        for wrapper in ["arm", "closure", "return"] {
            let source = |body: &str| {
                let branch =
                    format!("if x==0u32{{use crate::{target} as low;{body}}}else{{(0u32,0u32)}}");
                let body = match wrapper {
                    "arm" => format!("match Some(x){{Some(_)=>{{{branch}}},None=>(0u32,0u32)}}"),
                    "closure" => format!("let c=|x:u32|{{{branch}}};c(x)"),
                    "return" => format!("return {{{branch}}};"),
                    _ => unreachable!("the wrapper is enumerated"),
                };
                format!(
                    "fn good(x:u32)->u32{{x&15u32}}fn bad(x:u32)->u32{{panic!(\"{{x}}\")}}use crate::good as low;fn f(x:u32)->(u32,u32){{{body}}}"
                )
            };
            if target == "good" {
                scheduled(&source(FORWARD), &source(REVERSE));
            } else {
                ordered(&source(FORWARD), &source(REVERSE));
            }
        }
    }
}

#[test]
fn constant_item_retention_preserves_surviving_local_owners() {
    let source = |body: &str| {
        format!(
            "fn low(x:u32)->u32{{x&15u32}}fn f(x:u32)->(u32,u32){{const N:usize=2;fn empty(_bytes:[u8;N]){{}}mod vacant{{}};{body}}}"
        )
    };
    scheduled(&source(FORWARD), &source(REVERSE));
}

#[test]
fn retained_calls_keep_metadata_after_scalar_block_cloning() {
    let source = |body: &str| {
        format!("fn low(x:u32)->u32{{x&15u32}}fn f(x:u32)->(u32,(u32,u32)){{let mask=3u32;{body}}}")
    };
    scheduled(
        &source("let a=low(x);let b=low(x^1u32);(mask,(a,b))"),
        &source("let b=low(x^1u32);let a=low(x);(mask,(a,b))"),
    );
}

#[test]
fn borrowed_signature_cloning_keeps_const_expression_helper_origins() {
    let source =
        |body: &str| format!("const fn low(x:bool)->bool{{!x}}fn f(_bytes:[u8;{{{body}}}]){{}}");
    scheduled(
        &source("let a=low(false);let b=low(true);if a&b{1usize}else{0usize}"),
        &source("let b=low(true);let a=low(false);if a&b{1usize}else{0usize}"),
    );
}

#[test]
fn empty_owners_and_retained_local_modules_keep_scope_identity() {
    let source = |body: &str| {
        format!(
            "fn low(x:u32)->u32{{x&15u32}}fn f(x:u32)->(u32,u32){{{{}}mod vacant{{}};if x==0u32{{}}else{{}};mod local{{pub fn low(x:u32)->u32{{panic!(\"{{x}}\")}}}}{body}}}"
        )
    };
    ordered(
        &source("let a=local::low(x);let b=local::low(x^1u32);(a,b)"),
        &source("let b=local::low(x^1u32);let a=local::low(x);(a,b)"),
    );
}

#[test]
fn arm_block_unwrapping_keeps_complete_original_helper_cost() {
    let source = |body: &str| {
        format!(
            "fn f(p:bool)->(bool,bool){{match Some(p){{Some(h)=>{{let _=h;}},None=>{{}}}}{body}}}fn h(p:bool)->bool{{{}p}}fn k(p:bool)->bool{{{}p}}",
            "p;".repeat(8189),
            "false;".repeat(4)
        )
    };
    ordered(
        &source("let a=h(p);let b=k(!p);(a,b)"),
        &source("let b=k(!p);let a=h(p);(a,b)"),
    );
}

#[test]
fn original_proofs_survive_tail_return_folding_without_revalidation() {
    let source = |body: &str| {
        format!("fn low(x:u32)->u32{{return x&15u32;}}fn f(x:u32)->(u32,u32){{{body}}}")
    };
    scheduled(&source(FORWARD), &source(REVERSE));
}

#[test]
fn rejected_original_bindings_keep_calls_ordered_after_mutability_folding() {
    let source = |body: &str| {
        format!("fn low(x:u32)->u32{{let mut y=x;y&15u32}}fn f(x:u32)->(u32,u32){{{body}}}")
    };
    ordered(&source(FORWARD), &source(REVERSE));
}

#[test]
fn rejected_helpers_leave_unrelated_scalar_regions_active() {
    let source = |body: &str| {
        format!(
            "fn low(x:u32)->u32{{let mut y=x;y&15u32}}fn f(x:u32)->(u32,u32){{let _=low(x);{body}}}"
        )
    };
    scheduled(
        &source("let a=x&15u32;let b=x^1u32;(a,b)"),
        &source("let b=x^1u32;let a=x&15u32;(a,b)"),
    );
}

#[test]
fn clone_retention_keeps_distinct_callee_origins() {
    let source = |call: &str| {
        format!(
            "fn low(x:u32)->u32{{x&15u32}}fn high(x:u32)->u32{{x&15u32}}fn f(x:u32)->(u32,u32){{let mask=3u32;(mask,{call})}}"
        )
    };
    ordered(&source("low(x)"), &source("high(x)"));
}

#[test]
fn borrowed_normalization_keeps_shared_original_metadata_reusable() {
    let file: syn::File =
        syn::parse_str(&local_module(FORWARD, "panic!(\"{x}\")", "x&15u32")).unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &file)));
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "f" => Some(function),
            _ => None,
        })
        .unwrap();
    let function = context.function(&function.sig, &function.block).unwrap();
    let expected = function.canonicalize();
    for _ in 0..8 {
        assert_eq!(function.canonicalize(), expected);
    }
}

#[test]
fn empty_original_lexical_cutoff_keeps_helper_calls_ordered() {
    let source = |body: &str| {
        format!(
            "fn h(p:bool)->bool{{{}{};p}}fn k(p:bool)->bool{{p}}fn f(p:bool)->(bool,bool){{{body}}}",
            "{".repeat(65),
            "}".repeat(65)
        )
    };
    ordered(
        &source("let a=h(p);let b=k(!p);(a,b)"),
        &source("let b=k(!p);let a=h(p);(a,b)"),
    );
}

#[test]
fn incomplete_original_aliases_keep_standalone_calls_ordered() {
    let build = |body: &str| {
        let mut file: syn::File = syn::parse_str(&format!(
            "type T=u32;fn h(x:T)->T{{x}}fn f(x:u32)->(u32,u32){{{body}}}"
        ))
        .unwrap();
        let syn::Item::Type(alias) = &mut file.items[0] else {
            unreachable!("the first item is the alias")
        };
        let syn::Type::Path(path) = alias.ty.as_mut() else {
            unreachable!("the alias target is a path")
        };
        path.path.segments.clear();
        syn_canon::canonicalize(file)
    };
    assert_ne!(
        build("let a=h(x);let b=h(x^1u32);(a,b)"),
        build("let b=h(x^1u32);let a=h(x);(a,b)")
    );
}

#[test]
fn incomplete_punctuated_aliases_keep_primitive_widths_distinct() {
    let build = |width: &str| {
        let mut file: syn::File =
            syn::parse_str(&format!("type T={width};fn f(x:{width})->{width}{{x}}")).unwrap();
        let syn::Item::Type(alias) = &mut file.items[0] else {
            unreachable!("the first item is the alias")
        };
        let syn::Type::Path(path) = alias.ty.as_mut() else {
            unreachable!("the alias target is a path")
        };
        path.path
            .segments
            .push_punct(syn::token::PathSep::default());
        syn_canon::canonicalize(file)
    };
    assert_ne!(build("u32"), build("u64"));
}
